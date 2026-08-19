//! The normalized model every provider collapses into.
//!
//! Adding a provider means writing one adapter that emits a [`Provider`]; no
//! consumer of this crate should ever branch on `Provider::id`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

// `export_to` is resolved relative to the directory of *this source file*, not
// the crate root, which is why the path climbs three levels rather than two.
// `cargo test` regenerates the widget's `src/types/`; nothing there is
// hand-written.

#[derive(TS, Debug, Clone, Serialize, Deserialize)]
#[ts(export, export_to = "../../../widget/src/types/")]
pub struct Snapshot {
    pub generated_at: DateTime<Utc>,
    pub providers: Vec<Provider>,
}

#[derive(TS, Debug, Clone, Serialize, Deserialize)]
#[ts(export, export_to = "../../../widget/src/types/")]
pub struct Provider {
    pub id: String,
    pub label: String,
    /// Subscription tier as the provider names it: "pro", "plus", …
    pub plan: Option<String>,
    pub status: Status,
    /// How this provider's credential was obtained, so the UI can show it.
    pub auth: AuthKind,
    pub updated_at: DateTime<Utc>,
    pub meters: Vec<Meter>,
}

#[derive(TS, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
#[ts(export, export_to = "../../../widget/src/types/")]
pub enum Status {
    Ok,
    /// Last read succeeded but is older than we'd like — show the number, dimmed.
    Stale { since: DateTime<Utc> },
    /// Last read failed. Never render a number alongside this.
    Error { message: String },
    /// Structurally impossible for this provider (e.g. opencode has no balance API).
    Unavailable { reason: String },
}

#[derive(TS, Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../../widget/src/types/")]
pub enum AuthKind {
    /// We hold our own OAuth grant and refresh it ourselves.
    OwnGrant,
    /// We borrowed a CLI's credential read-only and will never refresh it.
    Delegated,
    ApiKey,
    None,
}

#[derive(TS, Debug, Clone, Serialize, Deserialize)]
#[ts(export, export_to = "../../../widget/src/types/")]
pub struct Meter {
    pub id: String,
    pub label: String,
    pub kind: MeterKind,
    pub severity: Severity,
}

#[derive(TS, Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export, export_to = "../../../widget/src/types/")]
pub enum MeterKind {
    /// A rolling usage window: the "28% of your 5 hours" case.
    Window {
        used_pct: f32,
        resets_at: Option<DateTime<Utc>>,
        window_mins: Option<u32>,
    },
    /// Money or credits remaining.
    Balance {
        amount: f64,
        currency: String,
        of_total: Option<f64>,
        unlimited: bool,
    },
    /// Money already spent over a period. Not a limit — just a total.
    Spend {
        amount: f64,
        currency: String,
        period: Period,
    },
}

#[derive(TS, Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../../widget/src/types/")]
pub enum Period {
    Rolling30d,
    AllTime,
}

#[derive(TS, Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../../widget/src/types/")]
pub enum Severity {
    Normal,
    Warning,
    Critical,
}

impl Severity {
    /// Thresholds are deliberately here rather than in each adapter, so every
    /// provider is judged on the same scale.
    pub fn from_pct(pct: f32) -> Self {
        match pct {
            p if p >= 95.0 => Severity::Critical,
            p if p >= 80.0 => Severity::Warning,
            _ => Severity::Normal,
        }
    }

    pub fn from_balance(remaining: f64, unlimited: bool) -> Self {
        if unlimited {
            return Severity::Normal;
        }
        match remaining {
            r if r <= 1.0 => Severity::Critical,
            r if r <= 5.0 => Severity::Warning,
            _ => Severity::Normal,
        }
    }
}

impl Meter {
    pub fn window(
        id: &str,
        label: &str,
        used_pct: f32,
        resets_at: Option<DateTime<Utc>>,
        window_mins: Option<u32>,
    ) -> Self {
        Meter {
            id: id.into(),
            label: label.into(),
            kind: MeterKind::Window {
                used_pct,
                resets_at,
                window_mins,
            },
            severity: Severity::from_pct(used_pct),
        }
    }

    pub fn balance(id: &str, label: &str, amount: f64, currency: &str, unlimited: bool) -> Self {
        Meter {
            id: id.into(),
            label: label.into(),
            kind: MeterKind::Balance {
                amount,
                currency: currency.into(),
                of_total: None,
                unlimited,
            },
            severity: Severity::from_balance(amount, unlimited),
        }
    }
}
