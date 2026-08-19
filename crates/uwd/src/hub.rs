//! Shared state: the latest reading per provider, a bounded history ring, and
//! the fan-out to connected viewers.
//!
//! Everything a viewer can see passes through here, so the rules about what a
//! failing provider looks like live here too rather than in each poll loop.

use std::collections::{BTreeMap, HashMap, VecDeque};

use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio::sync::{broadcast, RwLock};
use uw_core::model::{MeterKind, Provider, Severity, Snapshot, Status};

/// Consecutive failures tolerated before a tile stops showing its last known
/// numbers.
///
/// Below this we render the previous reading as `Stale`: a single failed poll
/// is usually a network blip, and blanking the panel every time one request
/// times out would make the widget useless. At and above it the numbers are old
/// enough that continuing to show them would be a lie, so the tile becomes
/// `Error` and drops its meters.
const FAILURES_BEFORE_ERROR: u32 = 3;

/// Something worth interrupting the user for. Only ever emitted on a severity
/// *increase* — see [`Hub::alerts_for`].
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export, export_to = "../../../widget/src/types/")]
pub struct Alert {
    pub at: DateTime<Utc>,
    pub provider: String,
    pub provider_label: String,
    pub meter: String,
    pub meter_label: String,
    pub severity: Severity,
    /// Ready to use as a notification body.
    pub message: String,
}

/// What goes out over SSE.
#[derive(Debug, Clone)]
pub enum Event {
    Snapshot(Snapshot),
    Alert(Alert),
}

pub struct Hub {
    inner: RwLock<Inner>,
    tx: broadcast::Sender<Event>,
    history_cap: usize,
}

struct Inner {
    /// Latest tile per provider, keyed by id so a re-poll replaces rather than
    /// appends. `BTreeMap` keeps the order stable across snapshots — a widget
    /// whose rows reshuffle on every update is unusable.
    providers: BTreeMap<String, Provider>,
    /// Consecutive failure count per provider.
    failures: HashMap<String, u32>,
    /// Last severity seen per `(provider, meter)`, for edge-triggering.
    severities: HashMap<(String, String), Severity>,
    history: VecDeque<Snapshot>,
}

impl Hub {
    pub fn new(history_cap: usize) -> Self {
        let (tx, _) = broadcast::channel(64);
        Hub {
            inner: RwLock::new(Inner {
                providers: BTreeMap::new(),
                failures: HashMap::new(),
                severities: HashMap::new(),
                history: VecDeque::new(),
            }),
            tx,
            history_cap,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }

    pub async fn snapshot(&self) -> Snapshot {
        let inner = self.inner.read().await;
        Snapshot {
            generated_at: Utc::now(),
            providers: inner.providers.values().cloned().collect(),
        }
    }

    /// Recent snapshots, oldest first, optionally trimmed to those after `since`.
    pub async fn history(&self, since: Option<DateTime<Utc>>) -> Vec<Snapshot> {
        let inner = self.inner.read().await;
        inner
            .history
            .iter()
            .filter(|s| since.is_none_or(|t| s.generated_at > t))
            .cloned()
            .collect()
    }

    /// Record a successful poll.
    pub async fn record(&self, provider: Provider) {
        let alerts;
        {
            let mut inner = self.inner.write().await;
            inner.failures.insert(provider.id.clone(), 0);
            alerts = inner.alerts_for(&provider);
            inner.providers.insert(provider.id.clone(), provider);
            let snap = Snapshot {
                generated_at: Utc::now(),
                providers: inner.providers.values().cloned().collect(),
            };
            inner.push_history(snap, self.history_cap);
        }
        self.broadcast_current(alerts).await;
    }

    /// Record a failed poll, degrading the tile according to how long it has
    /// been failing.
    pub async fn record_failure(&self, id: &str, label: &str, auth: uw_core::model::AuthKind, message: String) {
        {
            let mut inner = self.inner.write().await;
            let fails = inner.failures.entry(id.to_string()).or_insert(0);
            *fails += 1;
            let fails = *fails;

            match inner.providers.get_mut(id) {
                // We have a previous good reading and the outage is young:
                // keep the numbers, mark them stale.
                Some(p) if fails < FAILURES_BEFORE_ERROR && !p.meters.is_empty() => {
                    p.status = Status::Stale {
                        since: p.updated_at,
                    };
                }
                _ => {
                    inner.providers.insert(
                        id.to_string(),
                        uw_core::collect::error_tile(id, label, auth, message),
                    );
                    // The meters are gone, so the next recovery must be able to
                    // re-alert; forget what severities we had seen.
                    inner.severities.retain(|(p, _), _| p != id);
                }
            }

            let snap = Snapshot {
                generated_at: Utc::now(),
                providers: inner.providers.values().cloned().collect(),
            };
            inner.push_history(snap, self.history_cap);
        }
        self.broadcast_current(Vec::new()).await;
    }

    async fn broadcast_current(&self, alerts: Vec<Alert>) {
        let snap = self.snapshot().await;
        // A send error just means nobody is watching, which is normal.
        let _ = self.tx.send(Event::Snapshot(snap));
        for a in alerts {
            tracing::info!(provider = %a.provider, meter = %a.meter, "{}", a.message);
            let _ = self.tx.send(Event::Alert(a));
        }
    }
}

impl Inner {
    fn push_history(&mut self, snap: Snapshot, cap: usize) {
        if cap == 0 {
            return;
        }
        if self.history.len() >= cap {
            self.history.pop_front();
        }
        self.history.push_back(snap);
    }

    /// Edge-triggered: an alert fires only when a meter's severity is higher
    /// than the last one we saw for it.
    ///
    /// Level-triggering here would notify on every poll for as long as the
    /// meter stayed hot — once a minute, for hours. Dropping back to Normal
    /// re-arms it.
    fn alerts_for(&mut self, provider: &Provider) -> Vec<Alert> {
        let mut out = Vec::new();
        for meter in &provider.meters {
            let key = (provider.id.clone(), meter.id.clone());
            let previous = self.severities.insert(key, meter.severity);
            let rising = previous.is_none_or(|prev| meter.severity > prev);
            if rising && meter.severity != Severity::Normal {
                out.push(Alert {
                    at: Utc::now(),
                    provider: provider.id.clone(),
                    provider_label: provider.label.clone(),
                    meter: meter.id.clone(),
                    meter_label: meter.label.clone(),
                    severity: meter.severity,
                    message: describe(&provider.label, meter),
                });
            }
        }
        out
    }
}

fn describe(provider_label: &str, meter: &uw_core::model::Meter) -> String {
    match &meter.kind {
        MeterKind::Window { used_pct, .. } => {
            format!("{provider_label} {} is at {:.0}%", meter.label, used_pct)
        }
        MeterKind::Balance {
            amount, currency, ..
        } => format!(
            "{provider_label} {} down to {amount:.2} {currency}",
            meter.label
        ),
        MeterKind::Spend {
            amount, currency, ..
        } => format!(
            "{provider_label} {} at {amount:.2} {currency}",
            meter.label
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uw_core::model::{AuthKind, Meter};

    fn provider_at(pct: f32) -> Provider {
        Provider {
            id: "claude".into(),
            label: "Claude Code".into(),
            plan: Some("max".into()),
            status: Status::Ok,
            auth: AuthKind::OwnGrant,
            updated_at: Utc::now(),
            meters: vec![Meter::window("session", "5-hour", pct, None, Some(300))],
        }
    }

    #[tokio::test]
    async fn alerts_fire_once_per_rise_not_once_per_poll() {
        let hub = Hub::new(10);
        let mut rx = hub.subscribe();

        hub.record(provider_at(85.0)).await; // Normal -> Warning: alert
        hub.record(provider_at(86.0)).await; // still Warning: silent
        hub.record(provider_at(96.0)).await; // Warning -> Critical: alert

        let mut alerts = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            if let Event::Alert(a) = ev {
                alerts.push(a);
            }
        }
        assert_eq!(alerts.len(), 2, "got {alerts:#?}");
        assert_eq!(alerts[0].severity, Severity::Warning);
        assert_eq!(alerts[1].severity, Severity::Critical);
    }

    #[tokio::test]
    async fn dropping_back_to_normal_re_arms_the_alert() {
        let hub = Hub::new(10);
        let mut rx = hub.subscribe();

        hub.record(provider_at(85.0)).await;
        hub.record(provider_at(10.0)).await; // window reset
        hub.record(provider_at(85.0)).await; // must alert again

        let alerts: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok())
            .filter_map(|e| match e {
                Event::Alert(a) => Some(a),
                _ => None,
            })
            .collect();
        assert_eq!(alerts.len(), 2);
    }

    #[tokio::test]
    async fn a_brief_outage_keeps_the_numbers_but_marks_them_stale() {
        let hub = Hub::new(10);
        hub.record(provider_at(42.0)).await;

        hub.record_failure("claude", "Claude Code", AuthKind::OwnGrant, "timeout".into())
            .await;

        let snap = hub.snapshot().await;
        let p = &snap.providers[0];
        assert!(matches!(p.status, Status::Stale { .. }));
        assert_eq!(p.meters.len(), 1, "a blip must not blank the panel");
    }

    #[tokio::test]
    async fn a_sustained_outage_stops_showing_old_numbers() {
        let hub = Hub::new(10);
        hub.record(provider_at(42.0)).await;

        for _ in 0..FAILURES_BEFORE_ERROR {
            hub.record_failure("claude", "Claude Code", AuthKind::OwnGrant, "timeout".into())
                .await;
        }

        let snap = hub.snapshot().await;
        let p = &snap.providers[0];
        assert!(matches!(p.status, Status::Error { .. }));
        assert!(p.meters.is_empty(), "an errored tile must show no numbers");
    }

    #[tokio::test]
    async fn recovery_after_an_errored_tile_alerts_again() {
        let hub = Hub::new(10);
        hub.record(provider_at(85.0)).await;
        for _ in 0..FAILURES_BEFORE_ERROR {
            hub.record_failure("claude", "Claude Code", AuthKind::OwnGrant, "down".into())
                .await;
        }

        let mut rx = hub.subscribe();
        hub.record(provider_at(85.0)).await;

        let alerts: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok())
            .filter_map(|e| match e {
                Event::Alert(a) => Some(a),
                _ => None,
            })
            .collect();
        assert_eq!(alerts.len(), 1, "the outage cleared the remembered severity");
    }

    #[tokio::test]
    async fn history_is_bounded() {
        let hub = Hub::new(3);
        for _ in 0..10 {
            hub.record(provider_at(1.0)).await;
        }
        assert_eq!(hub.history(None).await.len(), 3);
    }
}
