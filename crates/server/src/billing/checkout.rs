//! `POST /billing/checkout` — the payment row, the discount, the student price, one QR.
//!
//! Four rules live here, and each is a rule rather than a step:
//!
//!  * **The platform check comes first** ([`super::checkout_allowed`]), and the platform
//!    is the *device row* (A144). A body that says `platform: "web"` from an `ios` device
//!    is still refused, because the body is the client's claim and the device row is a
//!    fact written at login.
//!  * **The amount comes from `billing::price`**, never from the body. It is the same
//!    function the settlement is later checked against, so the number quoted and the
//!    number verified cannot disagree. The one figure this module does *not* compute is the
//!    gateway's unique offset, which arrives with the invoice and is stored beside the price
//!    rather than replacing it (`payments.payable_amount_idr`).
//!  * **A second checkout for the same plan and period reuses the pending invoice**
//!    (A307) rather than minting a second QR: "the user paid twice for one period" is one
//!    of the spec's edge cases, and a second QR is how it happens.
//!  * **A gateway failure rolls the whole thing back.** A pending row with no QR is worse
//!    than no row: the student cannot pay it, and the poller would ask the gateway about a
//!    payment the gateway never created.

use chrono::Duration;
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use stvdy_core::billing::{BillingPeriod, DiscountCode, price};
use stvdy_core::config::PlanId;
use stvdy_core::traits::payment::InvoiceRequest;
use uuid::Uuid;

use crate::auth::extract::{AuthUser, VerifiedUser};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// What a checkout returns to the client, and what `GET /billing/payments/:id` polls.
///
/// `amount_idr` is the stored amount, and `amount_before_discount_idr` with
/// `discount_percent` is the itemisation behind it — the honesty rule (gate 7(b)) as a
/// field, so the screen can say "5,000 IDR, 50% student discount applied" from the row
/// rather than by re-deriving the arithmetic.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct PaymentView {
    pub id: Uuid,
    pub order_id: String,
    pub kind: String,
    pub plan_id: Option<String>,
    pub billing_period: Option<String>,
    pub credits: Option<i32>,
    pub amount_idr: i64,
    /// The amount to actually transfer: the price plus the gateway's unique offset.
    ///
    /// Always present, and always at least `amount_idr` — the field is coalesced to the
    /// price so a client never has to decide what a null means. The offset exists so the
    /// gateway can match a settlement to one order by amount alone; it is at most 999 IDR,
    /// which is why the checkout screen shows it as a line rather than as a second price.
    pub payable_amount_idr: i64,
    pub amount_before_discount_idr: i64,
    pub discount_percent: i32,
    pub discount_code: Option<String>,
    pub student_discount: bool,
    pub qr_string: Option<String>,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    pub status: String,
    pub paid_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// The one `select` that builds a [`PaymentView`], so a field cannot be present on the
/// checkout response and absent from the poll response.
///
/// A macro rather than a `format!`: sqlx refuses a dynamically built SQL string
/// (`SqlSafeStr`), and the right answer is not to silence it but to have one literal that
/// every caller concatenates at compile time.
macro_rules! payment_select {
    () => {
        // The `::bigint` casts are load-bearing. `payments.amount_idr` and
        // `amount_before_discount_idr` are `int` (INT4), and sqlx decodes a column by its
        // SQL type: an `i64` field against an INT4 column is a runtime
        // "mismatched types; Rust type `i64` is not compatible with SQL type `INT4`" on
        // **every** read, which is a 500 on every checkout. Money is `i64` in Rust (whole
        // rupiah, and a yearly power plan is 250000 — well inside INT4, but the arithmetic
        // in `billing::price` is i64 throughout), so the cast belongs here rather than
        // narrowing the field. The cast is *outside* the `coalesce`: `coalesce(int, int)` is
        // INT4 and would decode exactly as badly.
        "select id, order_id, kind, plan_id, billing_period, credits,
                amount_idr::bigint as amount_idr,
                coalesce(payable_amount_idr, amount_idr)::bigint as payable_amount_idr,
                coalesce(amount_before_discount_idr, amount_idr)::bigint
                    as amount_before_discount_idr,
                discount_percent, discount_code, student_discount, qr_string, expires_at,
                status, paid_at
           from payments"
    };
}

#[derive(Debug, Deserialize)]
pub struct CheckoutBody {
    /// A subscription checkout: the plan and the period.
    pub plan_id: Option<String>,
    pub billing_period: Option<String>,
    /// A credit-pack checkout: the pack is named by the credits it grants.
    pub credit_pack: Option<i64>,
    pub discount_code: Option<String>,
}

/// `POST /billing/checkout`
///
/// Takes `VerifiedUser`, so the gate is enforced by the extractor rather than by a line in
/// the handler a later edit could drop. Checkout is one of the routes A43 named first — a
/// verified address before any action that costs money — and A359 kept it there.
pub async fn create(
    state: &AppState,
    user: &VerifiedUser,
    body: CheckoutBody,
) -> AppResult<PaymentView> {
    // Phase 7's boundary, called first, with the platform from the device row (A144/A151).
    super::checkout_allowed(&state.config.billing_policy, &user.platform)?;

    // The gateway is resolved before anything is written, so a deployment with no gateway
    // answers 502 without leaving a row behind.
    state.payment()?;
    let qris = state.qris()?;

    let student = user.student_verified;
    let code = load_discount(state, body.discount_code.as_deref()).await?;

    let (kind, plan_id, period, credits, amount_idr, breakdown) =
        match (body.plan_id.as_deref(), body.credit_pack) {
            // A credit pack: the price comes from `[credit_packs]` in the config, because a
            // price invented in code is what A21 forbids. A pack takes no discount — the
            // student promo and a code are subscription discounts, and applying a percentage
            // to a pack would be a second pricing rule nobody wrote down.
            (None, Some(pack_credits)) => {
                let pack = state
                    .config
                    .plans
                    .credit_packs
                    .pack(pack_credits)
                    .ok_or_else(|| {
                        AppError::Validation(format!(
                            "no credit pack grants {pack_credits} credits"
                        ))
                    })?
                    .clone();
                (
                    "credit_pack",
                    None,
                    None,
                    Some(i32::try_from(pack.credits).unwrap_or(i32::MAX)),
                    pack.price_idr,
                    None,
                )
            }
            // A subscription.
            (Some(plan_id), None) => {
                let plan = PlanId::parse(plan_id);
                if plan == PlanId::Free {
                    return Err(AppError::Validation(
                        "the free plan is not for sale".to_string(),
                    ));
                }
                let period = body
                    .billing_period
                    .as_deref()
                    .and_then(BillingPeriod::parse)
                    .ok_or_else(|| {
                        AppError::Validation("billing_period must be monthly or yearly".to_string())
                    })?;
                let breakdown = price(state.plans().get(plan), period, student, code.as_ref());
                (
                    "subscription",
                    Some(plan.as_str()),
                    Some(period),
                    None,
                    breakdown.due_idr,
                    Some(breakdown),
                )
            }
            _ => {
                return Err(AppError::Validation(
                    "send either plan_id with billing_period, or credit_pack".to_string(),
                ));
            }
        };

    let mut tx =
        state.pool.begin().await.map_err(|error| {
            AppError::Internal(anyhow::anyhow!("beginning a checkout: {error}"))
        })?;

    let view = open_invoice(
        &mut tx,
        state,
        user,
        NewPayment {
            kind,
            plan_id,
            billing_period: period.map(BillingPeriod::as_str),
            credits,
            amount_idr,
            subscription_id: None,
            discount_code: breakdown.as_ref().and_then(|b| b.code_applied.as_deref()),
            discount_percent: breakdown.as_ref().map(|b| b.percent_off).unwrap_or(0) as i32,
            student_discount: breakdown.as_ref().is_some_and(|b| b.student_applied),
            amount_before_discount_idr: breakdown
                .as_ref()
                .map(|b| b.list_idr)
                .unwrap_or(amount_idr),
            // **The code is consumed only when it determined the price** (A298). Passing the
            // loaded code unconditionally would spend a once-per-user code on a checkout
            // where the student promo won the tie: the code buys nothing, and the *next*
            // checkout with it is then refused as "already used on this account". The tie
            // rule and the redemption rule have to agree, and this is where they do.
            discount: applied_code(breakdown.as_ref(), code.as_ref()),
        },
        qris.invoice_ttl_minutes,
    )
    .await?;

    tx.commit()
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("committing a checkout: {error}")))?;

    Ok(view)
}

/// The code that actually determined the price, or `None`.
///
/// `billing::price` already decided (A298: the larger percentage wins, and a tie goes to the
/// student promo); this is the one place that decision is turned into "was the code spent".
/// A code the promo beat must not be redeemed: it bought nothing, and spending it would
/// refuse the student's *next* checkout for a discount they never received.
fn applied_code<'a>(
    breakdown: Option<&stvdy_core::billing::PriceBreakdown>,
    code: Option<&'a DiscountCode>,
) -> Option<&'a DiscountCode> {
    let code = code?;
    match breakdown {
        Some(breakdown) if breakdown.code_applied.as_deref() == Some(code.code.as_str()) => {
            Some(code)
        }
        _ => None,
    }
}

/// The fields of a payment to open, as the two callers that open one supply them.
pub(crate) struct NewPayment<'a> {
    pub kind: &'a str,
    pub plan_id: Option<&'a str>,
    pub billing_period: Option<&'a str>,
    pub credits: Option<i32>,
    pub amount_idr: i64,
    pub subscription_id: Option<Uuid>,
    pub discount_code: Option<&'a str>,
    pub discount_percent: i32,
    pub student_discount: bool,
    pub amount_before_discount_idr: i64,
    /// The loaded code, for the redemption row. `None` when no code was supplied, and also
    /// when a code was supplied but the student promo won the tie (A298) — a code that
    /// bought nothing must not be consumed.
    pub discount: Option<&'a DiscountCode>,
}

/// Inserts the pending payment and mints its gateway invoice, **on the caller's
/// connection**.
///
/// One function for both callers — `create` and `periods::upgrade` — so the sequence
/// "insert, call the gateway, write the QR back" exists once. A second copy would be a
/// second place to forget the rollback, and the rollback is the rule: a pending row with
/// no QR is a student who cannot pay and a poller asking about an order the gateway never
/// created.
///
/// The A307 reuse rule is here too, so an upgrade gets it for free: a pending invoice for
/// the same `(user, kind, plan, period)` is returned instead of a second QR.
pub(crate) async fn open_invoice(
    conn: &mut PgConnection,
    state: &AppState,
    user: &AuthUser,
    new: NewPayment<'_>,
    ttl_minutes: i64,
) -> AppResult<PaymentView> {
    let gateway = state.payment()?;

    // A307: reuse rather than mint. `subscription_id` is deliberately not part of the
    // key — two renewals of one plan and period are one invoice.
    // The reuse key is every field that decides **what is being bought**: the user, the
    // kind, the plan, the period and the credit amount. `credits` is in it because a pack's
    // identity *is* the credits it grants (`CreditPack`'s own doc), and a credit-pack row
    // has `plan_id` and `billing_period` NULL — `is not distinct from` treats NULL as equal
    // to NULL, so without `credits` a student asking for a 500-credit pack would be handed
    // back the 100-credit invoice they already have open, and charged 3,000 IDR for 100
    // credits. That is the reuse rule silently selling the wrong product.
    let existing: Option<PaymentView> = sqlx::query_as(concat!(
        payment_select!(),
        " where user_id = $1 and kind = $2
            and plan_id is not distinct from $3
            and billing_period is not distinct from $4
            and credits is not distinct from $5
            and status = 'pending' and expires_at > now()
          order by created_at desc
          limit 1"
    ))
    .bind(user.user_id)
    .bind(new.kind)
    .bind(new.plan_id)
    .bind(new.billing_period)
    .bind(new.credits)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|error| {
        AppError::Internal(anyhow::anyhow!("looking for a pending invoice: {error}"))
    })?;

    if let Some(existing) = existing {
        return Ok(existing);
    }

    // The discount is consumed **in the same transaction that writes the payment**, so a
    // code cannot be counted twice by two concurrent checkouts (A298). The increment is
    // guarded by `max_redemptions`, and the redemption row is guarded by
    // `unique (code, user_id)`.
    if let Some(code) = new.discount {
        let incremented = sqlx::query(
            "update discount_codes
                set redeemed = redeemed + 1
              where code = $1
                and (valid_until is null or valid_until > now())
                and (max_redemptions is null or redeemed < max_redemptions)",
        )
        .bind(&code.code)
        .execute(&mut *conn)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("redeeming a code: {error}")))?;

        if incremented.rows_affected() == 0 {
            // Exhausted, expired, or never there. One message for all three: telling a
            // caller which of the three it was turns this route into a code oracle.
            return Err(AppError::Validation(
                "this discount code cannot be used".to_string(),
            ));
        }
    }

    let order_id = new_order_id();
    let expires_at = state.clock.now() + Duration::minutes(ttl_minutes);

    // The provider recorded on the row is the gateway that minted the invoice, never a
    // literal. `payments` is unique on `(provider, order_id)` and the poller resolves a
    // payment with the same `gateway.name()`, so a hardcoded `'qris_gateway'` would be a
    // lookup that cannot match the moment a second provider exists — and every status
    // question for it would answer "unknown" while the payment sat pending.
    let provider = gateway.name();

    let payment_id: Uuid = sqlx::query_scalar(
        "insert into payments
             (user_id, subscription_id, kind, plan_id, billing_period, credits, provider,
              order_id, amount_idr, amount_before_discount_idr, discount_percent,
              student_discount, discount_code, platform, expires_at, status)
         values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, 'pending')
         returning id",
    )
    .bind(user.user_id)
    .bind(new.subscription_id)
    .bind(new.kind)
    .bind(new.plan_id)
    .bind(new.billing_period)
    .bind(new.credits)
    .bind(provider)
    .bind(&order_id)
    .bind(new.amount_idr)
    .bind(new.amount_before_discount_idr)
    .bind(new.discount_percent)
    .bind(new.student_discount)
    .bind(new.discount_code)
    .bind(&user.platform)
    .bind(expires_at)
    .fetch_one(&mut *conn)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("writing the payment: {error}")))?;

    // The redemption row. `unique (code, user_id)` is what makes the code once per user,
    // enforced by the database rather than by a read-then-write in the route: two
    // concurrent checkouts both read "not redeemed yet", and only one insert survives.
    if let Some(code) = new.discount {
        let inserted = sqlx::query(
            "insert into discount_redemptions (code, user_id, payment_id)
             values ($1, $2, $3)
             on conflict (code, user_id) do nothing
             returning id",
        )
        .bind(&code.code)
        .bind(user.user_id)
        .bind(payment_id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("recording a redemption: {error}")))?;

        if inserted.is_none() {
            // The account already spent this code. The `update` above has already counted
            // the redemption, but this transaction rolls back with the error, so the count
            // is not left inflated.
            return Err(AppError::Validation(
                "this discount code has already been used on this account".to_string(),
            ));
        }
    }

    let description = match (new.kind, new.plan_id) {
        ("credit_pack", _) => format!("Stvdy {} credits", new.credits.unwrap_or(0)),
        (_, Some(plan)) => format!(
            "Stvdy {} ({})",
            plan,
            new.billing_period.unwrap_or("monthly")
        ),
        _ => "Stvdy purchase".to_string(),
    };

    // The gateway call is the last thing before the write-back, and a failure here returns
    // the error: the caller's transaction drops uncommitted, so there is no row and no
    // half-minted invoice.
    let invoice = gateway
        .create_invoice(InvoiceRequest {
            order_id: order_id.clone(),
            amount_idr: new.amount_idr,
            description,
            expires_at,
        })
        .await
        .map_err(|error| {
            tracing::error!(%error, order_id = %order_id, "the gateway refused to mint an invoice");
            AppError::Provider(format!(
                "the payment gateway could not create an invoice: {error}"
            ))
        })?;

    // The gateway's expiry is authoritative (A6/A306): it is the instant the code dies, and
    // the countdown the client renders is this value against the client's own clock.
    //
    // `payable_amount_idr` is written in the same statement, and it is the one number here
    // that did not come from us. The gateway adds a unique offset so it can match a
    // settlement by amount alone, so the figure the QR encodes is not the price — and if it
    // were not stored, the checkout screen would show a number the student must not
    // transfer and the amount check would flag every real payment. `None` (a gateway that
    // charges exactly what it is asked for) leaves the column NULL, and every reader falls
    // back to `amount_idr`.
    sqlx::query(
        "update payments
            set qr_string = $2, provider_payment_id = $3, expires_at = $4,
                payable_amount_idr = $5
          where id = $1",
    )
    .bind(payment_id)
    .bind(&invoice.qr_string)
    .bind(&invoice.provider_payment_id)
    .bind(invoice.expires_at)
    .bind(invoice.payable_amount_idr)
    .execute(&mut *conn)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("writing the QR back: {error}")))?;

    tracing::info!(
        %payment_id,
        order_id = %order_id,
        amount_idr = new.amount_idr,
        payable_amount_idr = invoice.payable_amount_idr.unwrap_or(new.amount_idr),
        kind = new.kind,
        platform = %user.platform,
        "checkout opened"
    );

    load_payment(conn, payment_id).await
}

/// `GET /billing/payments/:id` — the poll the checkout screen runs (A306).
///
/// Scoped by `user_id` in the statement rather than checked after the read: a payment id
/// is a UUID, but "scoped to the authenticated user" is a rule about the query, not about
/// how hard the id is to guess.
///
/// **Expiry is re-checked here**, so a client whose clock is wrong cannot extend its own
/// invoice by not noticing. A pending row past its expiry reads `expired`, and the poll
/// stops on every terminal status.
///
/// **And the gateway is asked, once, before the row is read.** The bridge never pushes, so
/// this poll is the fast ingress: without it a student who has paid waits for the
/// five-minute sweep to notice, which is a "waiting for confirmation" screen in front of
/// money that already moved (A301). It is asked *after* the expiry check, so a row this
/// request just expired is not resurrected by a late answer, and its result is only ever a
/// committed status — the read below is what the client sees either way.
pub async fn show(state: &AppState, user: &AuthUser, payment_id: Uuid) -> AppResult<PaymentView> {
    let mut conn =
        state.pool.acquire().await.map_err(|error| {
            AppError::Internal(anyhow::anyhow!("acquiring a connection: {error}"))
        })?;

    let expired = sqlx::query(
        "update payments set status = 'expired'
          where id = $1 and user_id = $2 and status = 'pending' and expires_at <= now()",
    )
    .bind(payment_id)
    .bind(user.user_id)
    .execute(&mut *conn)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("expiring the invoice: {error}")))?;

    if expired.rows_affected() > 0 {
        tracing::info!(%payment_id, "an invoice expired while the client was polling");
    }

    // The gateway call is scoped to the caller's own payment by the statement inside
    // `refresh_pending`, and a failure inside it is swallowed there: a bridge that is down
    // must not turn the checkout screen's poll into an error page.
    crate::billing::poll::refresh_pending(state, user.user_id, payment_id).await?;

    // The scope is in the statement, not in a check after the read: "every query scoped to
    // the authenticated user" is a rule about the query.
    let view: PaymentView = sqlx::query_as(concat!(
        payment_select!(),
        " where id = $1 and user_id = $2"
    ))
    .bind(payment_id)
    .bind(user.user_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("reading the payment: {error}")))?
    .ok_or(AppError::NotFound)?;

    Ok(view)
}

/// `GET /billing/invoices` — the caller's own payment history.
///
/// The billing screen's history panel has no client-side substitute: `payments` is
/// deliberately not synced (`docs/SPEC.md` line 137), so there is no device copy to read.
/// It is the same `select` as [`show`], scoped by `user_id` and bounded — a student with
/// years of history does not need it all on one screen, and the newest 50 is what a
/// payment-history panel renders.
///
/// Expiry is **not** re-checked here, unlike [`show`]: this is a read of what happened, and
/// a list that quietly rewrote rows while the student was looking at their history would be
/// a surprise. The screen's live invoice is the one it polls by id.
pub async fn list(state: &AppState, user: &AuthUser) -> AppResult<Vec<PaymentView>> {
    let invoices: Vec<PaymentView> = sqlx::query_as(concat!(
        payment_select!(),
        " where user_id = $1 order by created_at desc limit 50"
    ))
    .bind(user.user_id)
    .fetch_all(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("listing the invoices: {error}")))?;

    Ok(invoices)
}

/// The payment row, or `NotFound`.
async fn load_payment(conn: &mut PgConnection, payment_id: Uuid) -> AppResult<PaymentView> {
    let view: PaymentView = sqlx::query_as(concat!(payment_select!(), " where id = $1"))
        .bind(payment_id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("reading the payment: {error}")))?
        .ok_or(AppError::NotFound)?;
    Ok(view)
}

/// Loads a discount code, if one was supplied.
///
/// A code that does not exist, has expired or is exhausted is **not** an error here: it is
/// refused when it would actually be redeemed, and the two cases are different messages.
/// This is only the lookup that lets `billing::price` decide whether the code changes the
/// amount at all (A298's tie rule means a code can be supplied and buy nothing).
async fn load_discount(state: &AppState, code: Option<&str>) -> AppResult<Option<DiscountCode>> {
    let Some(code) = code.map(str::trim).filter(|code| !code.is_empty()) else {
        return Ok(None);
    };

    let row: Option<(String, i32)> = sqlx::query_as(
        "select code, percent_off from discount_codes
          where code = $1
            and (valid_until is null or valid_until > now())
            and (max_redemptions is null or redeemed < max_redemptions)",
    )
    .bind(code)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("loading a discount code: {error}")))?;

    let Some((code, percent_off)) = row else {
        return Err(AppError::Validation(
            "this discount code cannot be used".to_string(),
        ));
    };

    Ok(Some(DiscountCode {
        code,
        // Clamped for the same reason `billing::price` clamps: a row that says 150 is a
        // config mistake, and `u8` would wrap rather than saturate.
        percent_off: percent_off.clamp(0, 100) as u8,
    }))
}

/// The order id we send the gateway and store on the row.
///
/// `unique (provider, order_id)` is the constraint, so this must not repeat: a UUID v7 is
/// time-ordered (which keeps the index warm) and unpredictable (which keeps it from being
/// guessable by a callback forger). It is deliberately **not** derived from the payment id
/// or the user id: an order id that leaks either would let a callback name someone else's
/// order, and A309's third threat-model row is exactly that.
fn new_order_id() -> String {
    format!("stvdy_{}", Uuid::now_v7().simple())
}
