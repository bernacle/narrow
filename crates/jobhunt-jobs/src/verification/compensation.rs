//! Compensation as the authoritative source publishes it: whether it is
//! published, its ranges and their currency (only when the source says
//! which), and whether it changed since the previous verification.
//!
//! Nothing here judges pay. Currencies are never converted and a bare `$`
//! (or `¥`) is never read as a particular currency: USD, CAD, AUD, SGD,
//! MXN and others all write `$`.

use serde::{Deserialize, Serialize};

use crate::model::{Compensation, CompensationKind, PayInterval};

/// Symbols several currencies share.
const AMBIGUOUS_SYMBOLS: [&str; 2] = ["$", "¥"];

/// What is known about a range's currency.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CurrencyEvidence {
    /// The source gives an ISO 4217 code.
    Code { code: String },
    /// The source writes only a symbol several currencies use.
    Ambiguous { symbol: String },
    /// Nothing says.
    Unknown,
}

impl CurrencyEvidence {
    pub fn code(&self) -> Option<&str> {
        match self {
            Self::Code { code } => Some(code),
            _ => None,
        }
    }
}

/// One published pay range.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PayRange {
    /// `salary`, `bonus`, `equity_percentage`, …
    pub kind: String,
    /// The source's own label, which may name where the range applies
    /// ("Canada Annual Pay Range").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub currency: CurrencyEvidence,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval: Option<PayInterval>,
}

impl PayRange {
    /// "USD 140,000 – 180,000 per year", "$190,000 – 215,000 (currency
    /// unknown)".
    pub fn describe(&self) -> String {
        let amount = |v: f64| group(v);
        let range = match (self.min, self.max) {
            (Some(a), Some(b)) if (a - b).abs() < f64::EPSILON => amount(a),
            (Some(a), Some(b)) => format!("{} – {}", amount(a), amount(b)),
            (Some(a), None) => format!("from {}", amount(a)),
            (None, Some(b)) => format!("up to {}", amount(b)),
            (None, None) => "amount not given".to_owned(),
        };
        let money = match &self.currency {
            CurrencyEvidence::Code { code } => format!("{code} {range}"),
            CurrencyEvidence::Ambiguous { symbol } => {
                format!("{symbol}{range} (currency unknown: “{symbol}” is several currencies)")
            }
            CurrencyEvidence::Unknown if self.kind == "equity_percentage" => format!("{range}%"),
            CurrencyEvidence::Unknown => format!("{range} (currency not stated)"),
        };
        let per = match self.interval {
            Some(PayInterval::Hour) => " per hour",
            Some(PayInterval::Day) => " per day",
            Some(PayInterval::Week) => " per week",
            Some(PayInterval::Month) => " per month",
            Some(PayInterval::Year) => " per year",
            Some(PayInterval::OneTime) => " one-time",
            None => "",
        };
        let label = self
            .label
            .as_deref()
            .map(|l| format!(" — {l}"))
            .unwrap_or_default();
        let kind = match self.kind.as_str() {
            "salary" => String::new(),
            other => format!(" ({})", other.replace('_', " ")),
        };
        format!("{money}{per}{kind}{label}")
    }
}

impl PayRange {
    /// The same range, compact, for short messages: "USD 28–58/hr",
    /// "USD 120k–160k/yr", "BRL 25k–32k/mo". Presentation only: what
    /// [`PayRange::describe`] says, in fewer characters.
    pub fn compact(&self) -> String {
        let range = match (self.min, self.max) {
            (Some(a), Some(b)) if (a - b).abs() < f64::EPSILON => short(a),
            (Some(a), Some(b)) => format!("{}–{}", short(a), short(b)),
            (Some(a), None) => format!("from {}", short(a)),
            (None, Some(b)) => format!("up to {}", short(b)),
            (None, None) => "amount not given".to_owned(),
        };
        let money = match &self.currency {
            CurrencyEvidence::Code { code } => format!("{code} {range}"),
            CurrencyEvidence::Ambiguous { symbol } => format!("{symbol}{range} (currency unknown)"),
            CurrencyEvidence::Unknown if self.kind == "equity_percentage" => format!("{range}%"),
            CurrencyEvidence::Unknown => format!("{range} (currency not stated)"),
        };
        let per = match self.interval {
            Some(PayInterval::Hour) => "/hr",
            Some(PayInterval::Day) => "/day",
            Some(PayInterval::Week) => "/wk",
            Some(PayInterval::Month) => "/mo",
            Some(PayInterval::Year) => "/yr",
            Some(PayInterval::OneTime) => " one-time",
            None => "",
        };
        let kind = match self.kind.as_str() {
            "salary" => String::new(),
            other => format!(" ({})", other.replace('_', " ")),
        };
        let label = self
            .label
            .as_deref()
            .map(|l| format!(" ({l})"))
            .unwrap_or_default();
        format!("{money}{per}{kind}{label}")
    }
}

/// 58 → "58", 26.96 → "26.96", 120000 → "120k", 125500 → "125.5k",
/// 1200000 → "1.2M".
fn short(value: f64) -> String {
    let scaled = |v: f64, unit: &str| {
        let rounded = (v * 10.0).round() / 10.0;
        if (rounded - rounded.round()).abs() < f64::EPSILON {
            format!("{}{unit}", rounded.round() as u64)
        } else {
            format!("{rounded:.1}{unit}")
        }
    };
    if value >= 1_000_000.0 {
        scaled(value / 1_000_000.0, "M")
    } else if value >= 1_000.0 {
        scaled(value / 1_000.0, "k")
    } else if (value - value.round()).abs() > 0.001 {
        format!("{value}")
    } else {
        format!("{}", value.round() as u64)
    }
}

fn group(value: f64) -> String {
    let rounded = value.round() as u64;
    let digits = rounded.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if (value - value.round()).abs() > 0.001 {
        format!("{value}")
    } else {
        out
    }
}

/// Whether compensation is published.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompensationStatus {
    Published,
    NotPublished,
    /// The listing was not read (closed, unreachable), so nothing is known.
    NotObserved,
}

/// How compensation compares with the previous successful verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompensationChange {
    /// No earlier verification to compare with.
    FirstVerification,
    Unchanged,
    Changed,
    /// Not published before, published now.
    NewlyPublished,
    /// Published before, not now.
    Removed,
    /// Not observed at this attempt.
    NotObserved,
}

/// Compensation as verified at one attempt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompensationCheck {
    pub status: CompensationStatus,
    pub change: CompensationChange,
    /// The source's own summary text, verbatim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default)]
    pub ranges: Vec<PayRange>,
    /// At least one range's currency is only an ambiguous symbol.
    #[serde(default)]
    pub ambiguous_currency: bool,
    /// What was published, as the canonical model holds it, for comparing
    /// with the next verification.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed: Option<Compensation>,
    /// What the previous successful verification saw, when it changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous: Option<Compensation>,
}

impl CompensationCheck {
    pub fn not_observed() -> Self {
        Self {
            status: CompensationStatus::NotObserved,
            change: CompensationChange::NotObserved,
            summary: None,
            ranges: Vec::new(),
            ambiguous_currency: false,
            observed: None,
            previous: None,
        }
    }

    /// Reads what a source published, and compares it with `previous`:
    /// `None` when there is no earlier successful verification,
    /// `Some(None)` when that verification saw no compensation.
    pub fn observe(
        published: Option<&Compensation>,
        previous: Option<Option<&Compensation>>,
    ) -> Self {
        let (status, summary, ranges) = match published {
            Some(comp) if !comp.components.is_empty() || comp.summary.is_some() => (
                CompensationStatus::Published,
                comp.summary.clone(),
                ranges(comp),
            ),
            _ => (CompensationStatus::NotPublished, None, Vec::new()),
        };
        let published = published.filter(|_| status == CompensationStatus::Published);
        let change = match previous {
            None => CompensationChange::FirstVerification,
            Some(prev) => match (prev, published) {
                (None, None) => CompensationChange::Unchanged,
                (None, Some(_)) => CompensationChange::NewlyPublished,
                (Some(_), None) => CompensationChange::Removed,
                (Some(a), Some(b)) if a == b => CompensationChange::Unchanged,
                (Some(_), Some(_)) => CompensationChange::Changed,
            },
        };
        let previous = match (change, previous) {
            (CompensationChange::Changed | CompensationChange::Removed, Some(prev)) => {
                prev.cloned()
            }
            _ => None,
        };
        Self {
            status,
            change,
            ambiguous_currency: ranges
                .iter()
                .any(|r| matches!(r.currency, CurrencyEvidence::Ambiguous { .. })),
            summary,
            ranges,
            observed: published.cloned(),
            previous,
        }
    }
}

fn kind_name(kind: &CompensationKind) -> String {
    match kind {
        CompensationKind::Salary => "salary".into(),
        CompensationKind::EquityPercentage => "equity_percentage".into(),
        CompensationKind::EquityCashValue => "equity_cash_value".into(),
        CompensationKind::Bonus => "bonus".into(),
        CompensationKind::Commission => "commission".into(),
        CompensationKind::Other(other) => other.clone(),
    }
}

fn ranges(comp: &Compensation) -> Vec<PayRange> {
    let summary = comp.summary.as_deref().unwrap_or_default();
    let symbol = AMBIGUOUS_SYMBOLS
        .iter()
        .find(|s| summary.contains(**s))
        .map(|s| (*s).to_owned());
    comp.components
        .iter()
        .filter(|c| c.min.is_some() || c.max.is_some())
        .map(|c| {
            let monetary = !matches!(c.kind, CompensationKind::EquityPercentage);
            let currency = match (&c.currency, &symbol) {
                (Some(code), _) => CurrencyEvidence::Code {
                    code: code.to_uppercase(),
                },
                (None, Some(symbol)) if monetary => CurrencyEvidence::Ambiguous {
                    symbol: symbol.clone(),
                },
                (None, _) => CurrencyEvidence::Unknown,
            };
            PayRange {
                kind: kind_name(&c.kind),
                label: c.label.clone(),
                currency,
                min: c.min,
                max: c.max,
                interval: c.interval,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::CompensationComponent;

    fn range(code: &str, min: Option<f64>, max: Option<f64>, interval: PayInterval) -> PayRange {
        PayRange {
            kind: "salary".into(),
            label: None,
            currency: CurrencyEvidence::Code { code: code.into() },
            min,
            max,
            interval: Some(interval),
        }
    }

    #[test]
    fn compact_ranges_read_at_a_glance() {
        let hourly = range("USD", Some(28.0), Some(58.0), PayInterval::Hour);
        assert_eq!(hourly.describe(), "USD 28 – 58 per hour");
        assert_eq!(hourly.compact(), "USD 28–58/hr");
        let yearly = range("USD", Some(120_000.0), Some(160_000.0), PayInterval::Year);
        assert_eq!(yearly.compact(), "USD 120k–160k/yr");
        assert_eq!(
            range("USD", Some(60.0), Some(60.0), PayInterval::Hour).compact(),
            "USD 60/hr"
        );
        assert_eq!(
            range("BRL", Some(25_000.0), Some(32_000.0), PayInterval::Month).compact(),
            "BRL 25k–32k/mo"
        );
        assert_eq!(
            range("USD", Some(125_500.0), None, PayInterval::Year).compact(),
            "USD from 125.5k/yr"
        );
        assert_eq!(
            range("EUR", None, Some(1_200_000.0), PayInterval::Year).compact(),
            "EUR up to 1.2M/yr"
        );
        assert_eq!(
            range("USD", Some(26.96), Some(30.0), PayInterval::Hour).compact(),
            "USD 26.96–30/hr"
        );
        let labelled = PayRange {
            kind: "bonus".into(),
            label: Some("Canada Annual Pay Range".into()),
            ..yearly
        };
        assert_eq!(
            labelled.compact(),
            "USD 120k–160k/yr (bonus) (Canada Annual Pay Range)"
        );
    }

    fn salary(
        currency: Option<&str>,
        min: f64,
        max: f64,
        label: Option<&str>,
    ) -> CompensationComponent {
        CompensationComponent {
            kind: CompensationKind::Salary,
            label: label.map(str::to_owned),
            currency: currency.map(str::to_owned),
            min: Some(min),
            max: Some(max),
            interval: Some(PayInterval::Year),
        }
    }

    fn comp(summary: Option<&str>, components: Vec<CompensationComponent>) -> Compensation {
        Compensation {
            summary: summary.map(str::to_owned),
            components,
        }
    }

    #[test]
    fn explicit_codes_are_kept() {
        for code in ["USD", "CAD", "BRL"] {
            let c = comp(None, vec![salary(Some(code), 100_000.0, 150_000.0, None)]);
            let check = CompensationCheck::observe(Some(&c), None);
            assert_eq!(check.status, CompensationStatus::Published);
            assert_eq!(check.ranges[0].currency.code(), Some(code));
            assert!(!check.ambiguous_currency);
            assert_eq!(check.change, CompensationChange::FirstVerification);
        }
        let c = comp(None, vec![salary(Some("USD"), 140_000.0, 180_000.0, None)]);
        assert_eq!(
            CompensationCheck::observe(Some(&c), None).ranges[0].describe(),
            "USD 140,000 – 180,000 per year"
        );
    }

    #[test]
    fn a_bare_dollar_is_not_usd() {
        let c = comp(
            Some("$190K - $215K"),
            vec![CompensationComponent {
                kind: CompensationKind::Salary,
                label: None,
                currency: None,
                min: Some(190_000.0),
                max: Some(215_000.0),
                interval: None,
            }],
        );
        let check = CompensationCheck::observe(Some(&c), None);
        assert!(check.ambiguous_currency);
        assert_eq!(
            check.ranges[0].currency,
            CurrencyEvidence::Ambiguous { symbol: "$".into() }
        );
        assert_eq!(check.ranges[0].currency.code(), None);
        assert!(check.ranges[0].describe().contains("currency unknown"));
        assert_eq!(check.summary.as_deref(), Some("$190K - $215K"));
    }

    #[test]
    fn location_specific_ranges_keep_their_labels() {
        let c = comp(
            None,
            vec![
                salary(
                    Some("USD"),
                    200_000.0,
                    250_000.0,
                    Some("US Annual Pay Range"),
                ),
                salary(
                    Some("CAD"),
                    180_000.0,
                    220_000.0,
                    Some("Canada Annual Pay Range"),
                ),
            ],
        );
        let check = CompensationCheck::observe(Some(&c), None);
        assert_eq!(check.ranges.len(), 2);
        assert_eq!(
            check.ranges[1].label.as_deref(),
            Some("Canada Annual Pay Range")
        );
        assert!(
            check.ranges[1]
                .describe()
                .ends_with("— Canada Annual Pay Range")
        );
    }

    #[test]
    fn changes_are_classified() {
        let a = comp(None, vec![salary(Some("USD"), 100_000.0, 120_000.0, None)]);
        let b = comp(None, vec![salary(Some("USD"), 110_000.0, 130_000.0, None)]);
        let obs = |now: Option<&Compensation>, prev: Option<Option<&Compensation>>| {
            CompensationCheck::observe(now, prev)
        };
        assert_eq!(
            obs(Some(&a), Some(Some(&a))).change,
            CompensationChange::Unchanged
        );
        let changed = obs(Some(&b), Some(Some(&a)));
        assert_eq!(changed.change, CompensationChange::Changed);
        assert_eq!(changed.previous.as_ref(), Some(&a));
        assert_eq!(
            obs(Some(&a), Some(None)).change,
            CompensationChange::NewlyPublished
        );
        let removed = obs(None, Some(Some(&a)));
        assert_eq!(removed.change, CompensationChange::Removed);
        assert_eq!(removed.status, CompensationStatus::NotPublished);
        assert_eq!(obs(None, Some(None)).change, CompensationChange::Unchanged);
        assert_eq!(obs(None, None).status, CompensationStatus::NotPublished);
        assert_eq!(
            CompensationCheck::not_observed().status,
            CompensationStatus::NotObserved
        );
    }
}
