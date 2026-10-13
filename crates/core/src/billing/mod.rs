//! Pure billing arithmetic: what a student owes, and when their period ends.
//!
//! No database, no clock, no IO — every function here is a pure function of its
//! arguments, so it can be tested against fixed instants and prices instead of
//! against whatever the production tables happen to hold.
//!
//! It is one module for one reason: the amount the server quotes at checkout and the
//! amount the settlement is checked against come out of the same function, so a
//! customer cannot be charged one number and activated against another.

mod period;
mod price;

pub use period::{BillingPeriod, PeriodChange, apply_payment, upgrade_delta};
pub use price::{DiscountCode, PriceBreakdown, price};
