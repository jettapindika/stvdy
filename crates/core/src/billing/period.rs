//! The period rules: how long a payment buys, and what an upgrade costs mid-period.

use chrono::{DateTime, Duration, Utc};

use crate::config::PlanConfig;

use super::price::list_price;

/// The two prepaid periods the spec sells. There is no auto-renew: every renewal is a
/// new invoice (spec § Payments).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BillingPeriod {
    Monthly,
    Yearly,
}

impl BillingPeriod {
    pub const ALL: [BillingPeriod; 2] = [BillingPeriod::Monthly, BillingPeriod::Yearly];

    /// The number of days a period covers, and the denominator of every proration.
    ///
    /// The spec's own numbers — 30 and 365 — rather than calendar-month arithmetic:
    /// a period whose length depends on which month it started in would make February
    /// a different price.
    pub fn days(self) -> i64 {
        match self {
            BillingPeriod::Monthly => 30,
            BillingPeriod::Yearly => 365,
        }
    }

    /// The stored spelling (`payments.billing_period`, the checkout body).
    pub fn as_str(self) -> &'static str {
        match self {
            BillingPeriod::Monthly => "monthly",
            BillingPeriod::Yearly => "yearly",
        }
    }

    /// An unknown spelling is `None`, never a default: defaulting would silently sell
    /// whichever period the code happens to prefer.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "monthly" => Some(BillingPeriod::Monthly),
            "yearly" => Some(BillingPeriod::Yearly),
            _ => None,
        }
    }
}

/// What a payment does to a subscription's dates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeriodChange {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    /// The new period starts at the existing period's end rather than at the payment.
    pub extends: bool,
}

/// The period a payment buys (A296).
///
/// Live paid time — a `current_period_end` still in the future — is extended from that
/// end, so paying early never loses the days already paid for. Anything else starts a
/// fresh period at `paid_at`: an end in the past is not live, and extending from it
/// would hand the student a period with days already spent.
pub fn apply_payment(
    active_end: Option<DateTime<Utc>>,
    paid_at: DateTime<Utc>,
    period: BillingPeriod,
) -> PeriodChange {
    match active_end.filter(|end| *end > paid_at) {
        Some(end) => PeriodChange {
            start: end,
            end: end + Duration::days(period.days()),
            extends: true,
        },
        None => PeriodChange {
            start: paid_at,
            end: paid_at + Duration::days(period.days()),
            extends: false,
        },
    }
}

/// The prorated difference for the remaining days of the current period, in whole
/// rupiah.
///
/// Zero when the target plan is not more expensive — an upgrade that charges for a
/// downgrade is a refund request — and clamped at one period's difference, so a caller
/// that hands over more remaining days than the period has cannot charge for more time
/// than the period contains.
pub fn upgrade_delta(
    from: &PlanConfig,
    to: &PlanConfig,
    period: BillingPeriod,
    remaining_days: i64,
) -> i64 {
    let difference = list_price(to, period) - list_price(from, period);
    if difference <= 0 {
        return 0;
    }
    let remaining = remaining_days.clamp(0, period.days());
    difference * remaining / period.days()
}
