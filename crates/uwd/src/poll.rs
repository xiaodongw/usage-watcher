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
use chrono::{DateTime, Utc};
use uw_core::collect::Poller;
use uw_core::limits::RateLimited;
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
    ///
    /// Only the window that resets soonest gets a vote. This used to be "any
    /// window above zero", which read as "active" far more often than it
    /// should: Claude reports a 5-hour window beside two weekly ones, and the
    /// weeklies sit at 13% and 22% for days after a single request. So Claude
    /// polled at the active rate around the clock — sixty requests an hour,
    /// whether or not it had been touched since Tuesday — against an endpoint
    /// that rate limits by IP. The idle tier existed and never once applied.
    ///
    /// The soonest-resetting window is the one a fast poll could actually learn
    /// something from. A 7-day bucket at 13% says nothing about whether
    /// anything is being spent right now; a 5-hour window above zero says you
    /// used it within the last five hours, and it does fall back to zero
    /// overnight.
    fn for_reading(&self, p: &Provider) -> Duration {
        let windows: Vec<(Option<DateTime<Utc>>, f32)> = p
            .meters
            .iter()
            .filter_map(|m| match m.kind {
                MeterKind::Window {
                    used_pct,
                    resets_at,
                    ..
                } => Some((resets_at, used_pct)),
                // A balance only moves when something is spent, and we cannot
                // tell from one reading whether that is happening; treat it as
                // idle and let the window meters drive the pace.
                MeterKind::Balance { .. } | MeterKind::Spend { .. } => None,
            })
            .collect();

        let consuming = match windows
            .iter()
            .filter_map(|(at, pct)| at.map(|at| (at, *pct)))
            .min_by_key(|(at, _)| *at)
        {
            Some((_, used_pct)) => used_pct > 0.0,
            // No window says when it resets, so there is no "soonest" to pick.
            // Fall back to the old rule rather than calling it idle: a provider
            // that omits the timestamps should not silently poll slowly.
            None => windows.iter().any(|(_, pct)| *pct > 0.0),
        };

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

/// How long to wait after a failed poll, and whether the server chose it.
///
/// A server that says when to come back is obeyed even when that is longer
/// than our own backoff would ever grow to. Anthropic's usage endpoint asks for
/// an hour and [`MAX_BACKOFF`] is fifteen minutes, so without this the poller
/// knocks four times an hour for as long as the limit lasts — and on a limiter
/// that counts refused requests, that knocking is what keeps it lasting.
fn retry_delay(schedule: &Schedule, failures: u32, error: &anyhow::Error) -> (Duration, bool) {
    let asked = error
        .downcast_ref::<RateLimited>()
        .and_then(|limited| limited.retry_after);
    // Never *sooner* than the backoff: a 429 carrying `Retry-After: 1` during
    // an outage must not become a one-second retry loop.
    (
        asked.unwrap_or_default().max(schedule.backoff(failures)),
        asked.is_some(),
    )
}

/// How long to actually sleep.
///
/// Jitter is there to stop providers restarted together from staying in
/// lockstep, and it swings both ways — which on a wait the server named would
/// mean coming back up to 10% early and taking the same refusal again. Those
/// get a couple of seconds of margin instead, for the skew between their
/// deadline and our clock.
fn sleep_for(delay: Duration, honoured: bool) -> Duration {
    if honoured {
        delay + Duration::from_secs(2)
    } else {
        jitter(delay)
    }
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

        // `honoured` marks a wait the server named, which is not ours to
        // shorten — see the sleep below.
        let (delay, honoured) = match outcome {
            Ok(reading) => {
                failures = 0;
                let next = schedule.for_reading(&reading);
                tracing::debug!(provider = id, "polled, next in {}s", next.as_secs());
                hub.record(reading).await;
                (next, false)
            }
            Err(e) => {
                failures += 1;
                let message = format!("{e:#}");
                let (delay, honoured) = retry_delay(&schedule, failures, &e);
                if honoured {
                    tracing::info!(provider = id, "asked to wait {}s", delay.as_secs());
                }
                tracing::warn!(provider = id, failures, "poll failed: {message}");
                hub.record_failure(id, label, auth, message).await;
                (delay, honoured)
            }
        };

        tokio::select! {
            _ = tokio::time::sleep(sleep_for(delay, honoured)) => {}
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
    use chrono::{Duration as ChronoDuration, Utc};
    use uw_core::model::{AuthKind, Meter, Status};

    fn sched() -> Schedule {
        Schedule::from_secs((60, 300))
    }

    fn limited(retry_after: Option<u64>) -> anyhow::Error {
        RateLimited {
            status: uw_core::reqwest::StatusCode::TOO_MANY_REQUESTS,
            retry_after: retry_after.map(Duration::from_secs),
            what: "The Anthropic usage endpoint",
        }
        .into()
    }

    #[test]
    fn a_named_wait_beats_our_own_backoff_however_long_it_is() {
        // The case that prompted all of this: Anthropic asks for an hour, and
        // `MAX_BACKOFF` is fifteen minutes. Waiting fifteen would mean four
        // refusals an hour, every hour, forever.
        let (delay, honoured) = retry_delay(&sched(), 1, &limited(Some(3600)));
        assert_eq!(delay, Duration::from_secs(3600));
        assert!(honoured);
        assert!(delay > MAX_BACKOFF);
    }

    #[test]
    fn a_short_named_wait_does_not_shorten_the_backoff() {
        // A 429 carrying `Retry-After: 1` while a provider is down would
        // otherwise turn the backoff into a one-second hammer.
        let (delay, _) = retry_delay(&sched(), 4, &limited(Some(1)));
        assert_eq!(delay, sched().backoff(4));
    }

    #[test]
    fn an_ordinary_failure_still_backs_off_normally() {
        let (delay, honoured) = retry_delay(&sched(), 3, &anyhow::anyhow!("connection reset"));
        assert_eq!(delay, sched().backoff(3));
        assert!(!honoured, "nothing named a wait, so nothing to honour");
    }

    #[test]
    fn a_rate_limit_with_no_deadline_falls_back_to_the_backoff() {
        let (delay, honoured) = retry_delay(&sched(), 2, &limited(None));
        assert_eq!(delay, sched().backoff(2));
        assert!(!honoured);
    }

    #[test]
    fn a_named_wait_is_never_slept_through_early() {
        // Jitter swings ±10%, which on an hour is six minutes — six minutes
        // inside the window we were told to stay out of. An ordinary interval
        // still gets jittered, because that is what keeps four providers
        // restarted together from polling in lockstep forever.
        let hour = Duration::from_secs(3600);
        assert!(sleep_for(hour, true) >= hour);

        let minute = Duration::from_secs(60);
        let slept = sleep_for(minute, false);
        assert!(slept >= minute * 9 / 10 && slept <= minute * 11 / 10, "{slept:?}");
    }
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

    /// A Claude reading: a 5-hour window and two weekly ones, which is the
    /// shape that made the old rule wrong.
    fn claude_like(session_pct: f32, weekly_pct: f32) -> Provider {
        let hours = |n: i64| Some(Utc::now() + ChronoDuration::hours(n));
        with_meters(vec![
            Meter::window("session", "5-hour", session_pct, hours(3), None),
            Meter::window("weekly_all", "7-day", weekly_pct, hours(24 * 5), None),
            Meter::window("weekly_scoped", "weekly · Fable", weekly_pct, hours(24 * 5), None),
        ])
    }

    #[test]
    fn a_weekly_bucket_left_over_from_tuesday_does_not_count_as_busy() {
        // The bug this rule was written for. The weeklies stay at 13% for days
        // after a single request, so "any window above zero" meant Claude
        // polled at the active rate permanently — sixty requests an hour into
        // an endpoint that rate limits by IP.
        let p = claude_like(0.0, 13.0);
        assert_eq!(schedule().for_reading(&p), Duration::from_secs(300));
    }

    #[test]
    fn the_five_hour_window_still_decides_the_pace() {
        let p = claude_like(28.0, 13.0);
        assert_eq!(schedule().for_reading(&p), Duration::from_secs(60));
    }

    #[test]
    fn a_provider_that_dates_none_of_its_windows_still_polls_fast_when_busy() {
        // No timestamps means no "soonest", and guessing idle would quietly
        // halve the resolution of a provider that simply reports less.
        let p = with_meters(vec![Meter::window("w", "window", 40.0, None, None)]);
        assert_eq!(schedule().for_reading(&p), Duration::from_secs(60));
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
