-- Phase 13: the amount the buyer was told to transfer.
--
-- B1 is resolved: the gateway is a self-hosted QRIS bridge that assigns every invoice a
-- **unique payable amount** — the price plus an offset in [1, 999] — so that it can match
-- an incoming settlement to one order by amount alone, with no per-order callback. That
-- offset is the one number in the money path the server does not compute, and until this
-- column existed it had nowhere to live.
--
-- It cannot live in `amount_idr`. That column is `billing::price`'s output: the price the
-- plan matrix, the student promo and any discount code produce, and the number the
-- itemisation on the checkout screen is built from. Overwriting it with the gateway's
-- figure would leave a row that cannot explain why it disagrees with `config/plans.toml`,
-- which is the honesty rule this phase already keeps (gate 7(b)).
--
-- So the two numbers are kept apart, and each has exactly one reader:
--
--   * `amount_idr` is what was *bought* — the price. The itemisation, the margin report
--     and the revenue report all read it.
--   * `payable_amount_idr` is what was *charged* — the price plus the gateway's offset.
--     The QR encodes it, the checkout screen shows it as the total, and the amount check in
--     `billing::apply::apply_paid` compares the settlement against it.
--
-- Nullable, and the null is meaningful: a row written before this migration, and a gateway
-- that charges exactly what it is asked for (the test double), both leave it NULL, and every
-- reader falls back to `coalesce(payable_amount_idr, amount_idr)`.
--
-- Deliberately NOT a `+sync` migration, for the same reason 0035 is not: `payments` is a
-- billing internal that never reaches a client (`docs/SPEC.md` line 137), so
-- `0017_sync_triggers.sql` stays the last trigger-attaching migration and its array is
-- untouched.
alter table payments add column payable_amount_idr int;

-- Direction one of the reconciliation sweep joins a gateway transaction to our row **by
-- amount**, because the bridge's feed carries the provider's order id and not ours. The
-- index is `(provider, coalesce(...))` because that is the lookup exactly: a transaction
-- resolves to a payment only within the gateway that took the money, and two providers may
-- legitimately charge the same amount. It is partial because the sweep only ever asks
-- about live rows — a `refunded` or `review` payment is not a candidate — which is the same
-- shape as the poll index above.
create index on payments (provider, coalesce(payable_amount_idr, amount_idr))
  where status in ('pending', 'paid');

-- The callback table becomes the gateway-event table.
--
-- `0014_billing.sql` created `webhook_events` for a gateway that pushed signed callbacks.
-- That gateway does not exist: B1 resolved to a self-hosted QRIS bridge that is **polled**,
-- so every row in here is now an answer we read from the gateway's status route and
-- synthesised into an event — plus the admin "mark paid" tool's own record.
--
-- The rename is not cosmetic. The table's two columns are `event_id` and `payload`, and a
-- reader who finds a row with no `webhook` anywhere in the code would reasonably conclude
-- it is dead. It is not: it is the idempotency key that makes the poller's retries and the
-- client's poll collapse into one activation (A293), and it is the audit trail a disputed
-- payment is read from (A294). The name now says what writes it.
--
-- 0014 and 0035 keep their old spelling: an applied migration's text is checksummed by
-- sqlx and must never be edited, so the archaeology stays and this line is the rename.
alter table webhook_events rename to gateway_events;
alter index webhook_events_provider_event_id_key rename to gateway_events_provider_event_id_key;
