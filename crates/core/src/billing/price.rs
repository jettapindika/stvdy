//! What one plan costs, and why.

use crate::config::PlanConfig;

use super::period::BillingPeriod;

/// A discount code, as the checkout route loads it from `discount_codes`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscountCode {
    pub code: String,
    pub percent_off: u8,
}

/// The itemised answer: the list price, the discount that was applied, the amount due
/// and which of the two discounts produced it.
///
/// The two flags exist because the invoice and the `payments` row both have to say
/// *why* the amount is what it is: a student who also has a code must be able to see
/// that the code bought them nothing, and a support question about an unexpected
/// amount is answered by this struct rather than by re-deriving the arithmetic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PriceBreakdown {
    pub list_idr: i64,
    pub percent_off: u8,
    pub discount_idr: i64,
    pub due_idr: i64,
    /// The student promo determined the discount. `false` when the address is not
    /// verified, and also when the promo is worth nothing (a 0% discount is not an
    /// applied discount).
    pub student_applied: bool,
    /// The code that determined the discount, named so the invoice can print it.
    /// `None` when the code did not change the price — including a tie, where the
    /// student promo wins.
    pub code_applied: Option<String>,
}

/// The one function that computes what a student owes.
///
/// `code` and `student` **do not stack** (A298): the larger percentage wins. The rule
/// lives here rather than in a caller because a second caller would be a second answer
/// to "what does this cost" — and stacking two percentages is the kind of generosity
/// nobody notices until the margin report does.
///
/// The amount is whole rupiah with the discount truncated (`list * percent / 100`),
/// the same integer arithmetic `upgrade_delta` uses, so there is one rounding
/// convention in this module rather than two.
pub fn price(
    plan: &PlanConfig,
    period: BillingPeriod,
    student: bool,
    code: Option<&DiscountCode>,
) -> PriceBreakdown {
    let list_idr = list_price(plan, period);

    let student_percent = if student { plan.student_percent_off } else { 0 };
    let code_percent = code.map(|code| code.percent_off).unwrap_or(0);

    // Clamped: a `discount_codes` row that says 150% is a config mistake, and the safe
    // direction is "free" rather than an amount we owe the student.
    let percent_off = student_percent.max(code_percent).min(100);

    // A tie goes to the student promo, which needs no redemption: spending a
    // once-per-user code on a discount the student already has would buy nothing and
    // could not be undone.
    let student_applied = student_percent > 0 && student_percent >= code_percent;
    let code_applied = if code_percent > student_percent {
        code.map(|code| code.code.clone())
    } else {
        None
    };

    let discount_idr = list_idr * i64::from(percent_off) / 100;

    PriceBreakdown {
        list_idr,
        percent_off,
        discount_idr,
        due_idr: list_idr - discount_idr,
        student_applied,
        code_applied,
    }
}

/// The list price for a period, read from the plan and never derived from the other
/// period: a yearly price is the config's number, not twelve monthlies.
pub(crate) fn list_price(plan: &PlanConfig, period: BillingPeriod) -> i64 {
    match period {
        BillingPeriod::Monthly => plan.price_idr_monthly,
        BillingPeriod::Yearly => plan.price_idr_yearly,
    }
}
