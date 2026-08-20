//! One task per provider, each on its own schedule.
//!
//! Deliberately not a single loop over all providers: they have different
//! natural rhythms (Claude's 5-hour window moves in minutes, Codex publishes a
//! 7-day bucket), and a shared loop would either poll Codex far too often or
//! Claude far too rarely. Independent tasks also mean one provider backing off
//! after an outage never slows the others down.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use uw_core::collect::Poller;
use uw_core::model::{MeterKind, Provider};

use crate::hub::Hub;

/// Longest gap between polls while a provider is failing.
const MAX_BACKOFF: Duration = Duration::from_secs(900);

pub struct Schedule {
    pub active: Duration,
    pub idle: Duration,
}

impl Schedule {
    pub fn from_secs((active, idle): (u64, u64)) -> Self {
        Schedule {
            active: Duration::from_secs(active),
            idle: Duration::from_secs(idle),
        }
    }

    /// A provider is "active" when something is actually being consumed, which
    /// is the only time a fast poll tells us anything new.
    fn for_reading(&self, p: &Provider) -> Duration {
        let consuming = p.meters.iter().any(|m| match m.kind {
            MeterKind::Window { used_pct, .. } => used_pct > 0.0,
            // A balance only moves when something is spent, and we cannot tell
            // from one reading whether that is happening; treat it as idle and
            // let the window meters drive the pace.
            MeterKind::Balance { .. } | MeterKind::Spend { .. } => false,
        });
        if consuming {
            self.active
        } else {
            self.idle
        }
    }

    /// Exponential, capped. `failures` is 1 on the first failed poll.
    fn backoff(&self, failures: u32) -> Duration {
        let factor = 2u32.saturating_pow(failures.saturating_sub(1).min(10));
        self.active.saturating_mul(factor).min(MAX_BACKOFF)
    }
}

/// ±10%, so N providers restarted together do not stay in lockstep forever.
fn jitter(d: Duration) -> Duration {
    let millis = d.as_millis() as u64;
    let spread = millis / 10;
    if spread == 0 {
        return d;
    }
    // Cheap and good enough for scheduling: nanosecond noise from the clock.
    let noise = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|t| t.subsec_nanos() as u64)
        .unwrap_or(0);
    Duration::from_millis(millis - spread + (noise % (spread * 2 + 1)))
}

/// Poll one provider until shutdown.
pub async fn run(
    poller: Poller,
    hub: Arc<Hub>,
    http: uw_core::reqwest::Client,
    schedule: Schedule,
    mut shutdown: watch::Receiver<bool>,
) {
    let id = poller.id();
    let label = poller.label();
    let auth = poller.auth_kind();
    let mut failures: u32 = 0;

    loop {
        // Shutdown races the poll itself, not just the sleep. Two reasons: a
        // provider removed in the UI must not have an in-flight request write
        // its tile back into the hub afterwards, and the caller doing the
        // removing is an HTTP handler that cannot sit through a 20-second
        // timeout waiting for this task to notice.
        let outcome = tokio::select! {
            r = poller.poll(&http) => r,
            _ = shutdown.changed() => {
                tracing::debug!(provider = id, "stopping mid-poll");
                return;
            }
        };

        let delay = match outcome {
            Ok(reading) => {
                failures = 0;
                let next = schedule.for_reading(&reading);
                tracing::debug!(provider = id, "polled, next in {}s", next.as_secs());
                hub.record(reading).await;
                next
            }
            Err(e) => {
                failures += 1;
                let message = format!("{e:#}");
                tracing::warn!(provider = id, failures, "poll failed: {message}");
                hub.record_failure(id, label, auth, message).await;
                schedule.backoff(failures)
            }
        };

        tokio::select! {
            _ = tokio::time::sleep(jitter(delay)) => {}
            _ = shutdown.changed() => {
                tracing::debug!(provider = id, "stopping");
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use uw_core::model::{AuthKind, Meter, Status};

    fn schedule() -> Schedule {
        Schedule::from_secs((60, 300))
    }

    fn with_meters(meters: Vec<Meter>) -> Provider {
        Provider {
            id: "claude".into(),
            label: "Claude Code".into(),
            plan: None,
            status: Status::Ok,
            auth: AuthKind::OwnGrant,
            updated_at: Utc::now(),
            meters,
        }
    }

    #[test]
    fn a_window_in_use_polls_at_the_active_rate() {
        let p = with_meters(vec![Meter::window("s", "5-hour", 12.0, None, None)]);
        assert_eq!(schedule().for_reading(&p), Duration::from_secs(60));
    }

    #[test]
    fn an_untouched_window_polls_at_the_idle_rate() {
        let p = with_meters(vec![Meter::window("s", "5-hour", 0.0, None, None)]);
        assert_eq!(schedule().for_reading(&p), Duration::from_secs(300));
    }

    #[test]
    fn a_tile_with_no_meters_is_idle_not_busy() {
        assert_eq!(schedule().for_reading(&with_meters(vec![])), Duration::from_secs(300));
    }

    #[test]
    fn backoff_doubles_and_then_stops() {
        let s = schedule();
        assert_eq!(s.backoff(1), Duration::from_secs(60));
        assert_eq!(s.backoff(2), Duration::from_secs(120));
        assert_eq!(s.backoff(3), Duration::from_secs(240));
        // Capped rather than growing without bound, and no overflow panic at
        // absurd failure counts.
        assert_eq!(s.backoff(20), MAX_BACKOFF);
        assert_eq!(s.backoff(u32::MAX), MAX_BACKOFF);
    }

    #[test]
    fn jitter_stays_within_ten_percent() {
        let base = Duration::from_secs(100);
        for _ in 0..200 {
            let j = jitter(base);
            assert!(j >= Duration::from_secs(90) && j <= Duration::from_secs(110), "{j:?}");
        }
    }
}
