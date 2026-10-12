use crate::config::PlanId;

/// The account's credit balance, in the two buckets A9 pins: the monthly grant, spent
/// first, and purchased credit-pack credits, which never expire.
///
/// Distinct from `config::Credits`, which is the *cost table* — reusing that type would
/// put `chat_message = 1` where a balance belongs (A141).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Credits {
    pub monthly: i64,
    pub purchased: i64,
}

impl Credits {
    pub fn total(self) -> i64 {
        self.monthly + self.purchased
    }
}

/// The subscription's state, as the entitlement decision sees it.
///
/// `None` is an account with no subscription row at all (free), which is not the same
/// fact as an `Expired` one and must not be collapsed into it: only `Expired` has a
/// period that ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubStatus {
    None,
    Active,
    Grace,
    Expired,
    Canceled,
}

impl SubStatus {
    /// The statuses `subscriptions.status` holds (migration 0014).
    ///
    /// An unrecognised string is `None` and is logged by the caller: a status the code
    /// does not understand must not grant paid limits (A146).
    pub fn parse(raw: &str) -> SubStatus {
        match raw {
            "active" => SubStatus::Active,
            "grace" => SubStatus::Grace,
            "expired" => SubStatus::Expired,
            "canceled" => SubStatus::Canceled,
            _ => SubStatus::None,
        }
    }

    /// Whether this status still honours `users.plan_id`.
    ///
    /// This is the whole of A146: an expired subscription drops the account to the free
    /// limits without anyone editing `users.plan_id`, and a status the code has never
    /// heard of drops it too.
    pub fn grants_paid_limits(self) -> bool {
        matches!(self, SubStatus::Active | SubStatus::Grace)
    }
}

/// Everything `check` is allowed to look at.
///
/// Counters are the caller's, already scoped to the right local day/month through
/// `chrono-tz` (A6). The matrix itself is *not* here: it is `config/plans.toml`, handed
/// to `check_with` by the caller, so this stays a cheap per-request bundle and the
/// engine cannot hold a second copy of the config.
///
/// `plan` is the **effective** plan: `users.plan_id` is honoured only while
/// `subscription_status` grants paid limits (A146), and that derivation is the caller's.
/// The status is carried rather than consulted so a refusal can be explained and the
/// usage view can show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageSnapshot {
    pub plan: PlanId,
    pub subscription_status: SubStatus,
    pub credits: Credits,
    pub storage_bytes: i64,
    pub workspaces: i64,
    pub sources: i64,
    pub devices: i64,
    pub chat_msgs_today: i64,
    pub web_searches_today: i64,
    pub flashcards_this_month: i64,
    pub quizzes_this_month: i64,
    pub reports_this_month: i64,
    pub syllabus_imports: i64,
    /// The live Google connections, for [`Feature::GoogleAccount`]'s cap (A245).
    ///
    /// Counted from `calendar_connections` rows with a non-null `refresh_token_enc`, which
    /// is what makes the cap mean "connected accounts" rather than "rows ever created": a
    /// revoked or half-finished row is not an account the student is using.
    pub google_accounts: i64,
}

/// The plan-dependent facts `check` needs that are not counters: the costs, and whether
/// an upgrade may be offered to this client at all (A143).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlanFacts {
    /// False on an iOS or Android device, where `config/billing_policy.toml` forbids a
    /// purchase call to action (A24/A106): a locked feature is then `FeatureLocked`
    /// rather than `PlanRequired`. It comes from the device row, never the request body
    /// (A144).
    pub upgrade_offerable: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unrecognised_subscription_status_grants_nothing() {
        // A146's control: the effective plan is free unless the status is one of the two
        // that pay. A typo in the column, or a status a later phase adds, must land on
        // the free limits rather than on paid ones.
        assert_eq!(SubStatus::parse("active"), SubStatus::Active);
        assert_eq!(SubStatus::parse("grace"), SubStatus::Grace);
        assert_eq!(SubStatus::parse("expired"), SubStatus::Expired);
        assert_eq!(SubStatus::parse("canceled"), SubStatus::Canceled);
        assert_eq!(SubStatus::parse("pending"), SubStatus::None);
        assert_eq!(SubStatus::parse(""), SubStatus::None);

        assert!(SubStatus::Active.grants_paid_limits());
        assert!(SubStatus::Grace.grants_paid_limits());
        assert!(!SubStatus::None.grants_paid_limits());
        assert!(!SubStatus::Expired.grants_paid_limits());
        assert!(!SubStatus::Canceled.grants_paid_limits());
    }

    #[test]
    fn the_balance_is_both_buckets() {
        // Monthly is spent first (A9), but affordability is the sum: a user with an
        // empty monthly bucket and purchased credits can still act.
        assert_eq!(Credits::default().total(), 0);
        assert_eq!(
            Credits {
                monthly: 3,
                purchased: 7
            }
            .total(),
            10
        );
        assert_eq!(
            Credits {
                monthly: -5,
                purchased: 5
            }
            .total(),
            0
        );
    }
}
