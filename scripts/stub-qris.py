#!/usr/bin/env python3
"""A stub `gopay-bridge`, for the Phase 13 smoke run and nothing else.

**B1 is resolved and the server speaks the real gateway**, so this file is not a guess at a
vendor's wire. It is a *scriptable* stand-in for a bridge the run cannot make answer on
demand: the bridge never pushes — every outcome is read from its own status route — so the
only way a local run can produce "the buyer paid" is to be the thing the server asks. This
stub speaks the surface `crates/server/src/integrations/payment/mapping.rs` maps, field for
field, and nothing else; when the bridge's API changes, `mapping.rs` changes and this file
changes with it.

Four routes, the four the client calls:

  POST /payments                           -> an invoice, derived from the request
  GET  /payment/<id>                       -> the payment's state (scripted, `pending` by default)
  GET  /transactions?merchant_id=&minutes= -> the settled transactions (the paid ones it minted)

And the control surface the smoke script drives:

  POST /_control/status                    -> script the answer `GET /payment/<id>` gives
  POST /_control/transactions              -> settlements to report that this stub did not mint
  POST /_control/reset                     -> forget every scripted answer
  GET  /_control/requests                  -> everything this stub was asked, in order

Four properties are deliberate:

  * **The invoice is derived from the request**, not canned. `qrString` is
    `STUBQRIS:<reference>:<uniqueAmount>` and `expiresAt` is the requested lifetime against
    this process's clock, so a smoke run can assert that the code a student scans is the
    gateway's own derivation of *this* request. A canned invoice would let the reference, the
    price or the offset be wrong without the run noticing.
  * **The payable amount is derived, and unique among the payments still live.** The bridge
    adds an offset in `[1, 999]` to the amount it is asked for and matches an incoming
    settlement to one order **by amount alone** — so two live invoices with the same payable
    amount would make that impossible. The offset is a function of the amount and a counter
    rather than a random number, so the same sequence of checkouts produces the same figures
    and a failure is reproducible.
  * **A status word is echoed, never invented.** `GET /payment/<id>` answers with the word the
    run scripted, whatever it is: a typo is the server's own `Invalid` rather than a stub that
    quietly agrees. An unknown *path* is a 404, which is what the bridge answers for a payment
    it never minted (and what the server turns into `UnknownOrder`).
  * **There is no auth header check, because the bridge has none.** It publishes no host port
    and authenticates nothing (A292); a stub that demanded a credential would fail the real
    client rather than model it.
"""

import argparse
import json
import sys
import threading
from datetime import datetime, timedelta, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse

# Every payment this stub has minted, by the id it minted: the invoice it answered with, plus
# what the run has scripted since. This is the stub's own ledger, and `GET /transactions`
# reports the paid ones out of it.
PAYMENTS: dict[str, dict] = {}
# The answers the run has scripted, by gateway payment id. Absent means `pending`.
STATUSES: dict[str, dict] = {}
# Settlements to report that this stub did not mint: the "money arrived with no row behind it"
# case, and an amount the server *does* hold a row for.
EXTRA_TRANSACTIONS: list[dict] = []
# Everything this stub has been asked, in order: `{"method", "path", "query", "body"}`.
REQUESTS: list[dict] = []
LOCK = threading.Lock()

# How many invoices this stub has minted. The offset below is a function of it, so a run is
# reproducible.
MINTED = 0

LOG_PATH: str | None = None


def rfc3339(instant: datetime) -> str:
    """The wire form the bridge serialises its instants in (`time.RFC3339`)."""
    return (
        instant.astimezone(timezone.utc)
        .replace(microsecond=0)
        .isoformat()
        .replace("+00:00", "Z")
    )


def unique_amount(amount: int, seq: int) -> int:
    """`amount` plus an offset in `[1, 999]`, unique among the payments still live.

    Derived from the amount and the mint counter rather than drawn at random, so the same
    sequence of checkouts produces the same payable amounts. The probe is what makes it
    *unique*: the bridge's whole reason for the offset is that it matches a settlement to one
    order by amount alone, so two live invoices with the same payable amount would make that
    impossible. Called with `LOCK` held.
    """
    live = {payment["uniqueAmount"] for payment in PAYMENTS.values() if payment["status"] == "pending"}
    offset = (amount + seq) % 999 + 1
    for _ in range(999):
        if amount + offset not in live:
            return amount + offset
        offset = offset % 999 + 1
    raise RuntimeError("no free offset left in 1..999")


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *args):  # noqa: ARG002
        """Quiet: the smoke script reports what matters, not the access log."""

    def _send(self, status: int, payload) -> None:
        body = json.dumps(payload).encode("utf-8")
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _read_body(self) -> bytes:
        length = int(self.headers.get("content-length") or 0)
        return self.rfile.read(length) if length else b""

    def _record(self, method: str, path: str, query: dict, body) -> None:
        entry = {"method": method, "path": path, "query": query, "body": body}
        with LOCK:
            REQUESTS.append(entry)
        if LOG_PATH:
            with open(LOG_PATH, "a", encoding="utf-8") as handle:
                handle.write(json.dumps(entry) + "\n")

    def _status_object(self, payment: dict, scripted: dict | None) -> dict:
        """The bridge's view of one payment: the invoice, plus the scripted answer."""
        answer = {key: value for key, value in payment.items() if key != "settled"}
        if scripted is None:
            return answer

        answer["status"] = scripted["status"]
        if scripted["status"] == "paid":
            # The figure that actually landed and the instant it landed. `uniqueAmount` is
            # what the server checks the invoice against, so a scripted amount is how the run
            # drives "the settlement is not the payable". The `transaction` object is
            # **required** on a paid answer — the server refuses one without a settlement
            # instant — so it is always attached.
            landed = scripted.get("uniqueAmount", payment["uniqueAmount"])
            answer["uniqueAmount"] = landed
            answer["transaction"] = {
                "id": f"tx-{payment['id']}",
                "amount": landed,
                "time": scripted["paid_at"],
            }
        return answer

    def do_GET(self):  # noqa: N802
        parsed = urlparse(self.path)
        path = parsed.path
        query = {key: values[0] for key, values in parse_qs(parsed.query).items()}

        if path == "/_control/requests":
            with LOCK:
                self._send(200, {"requests": list(REQUESTS)})
            return

        self._record("GET", path, query, None)

        if path == "/transactions":
            # A **bare array**, which is what the bridge answers with and what
            # `mapping::transactions_from_response` requires. Every entry is money that
            # arrived, and `amount` is the figure the bridge matches settlements by — the
            # invoice's own `uniqueAmount`, never the price: an entry for the price would name
            # an amount the server holds no row for, which is a *different* case and one the
            # run injects deliberately if it wants it. The scripted `uniqueAmount` of a paid
            # answer is not reflected here either: it is the run driving the amount check, not
            # a settlement.
            with LOCK:
                settled = [
                    {
                        "id": f"tx-{index}",
                        "orderId": f"gopay-{index}",
                        "status": "settlement",
                        "paymentType": "qris",
                        "amount": payment["uniqueAmount"],
                        "realAmount": payment["amount"],
                        "currency": "IDR",
                        "time": payment["settled"]["time"],
                    }
                    for index, payment in enumerate(PAYMENTS.values(), start=1)
                    if payment["status"] == "paid" and payment["settled"]
                ] + list(EXTRA_TRANSACTIONS)
            self._send(200, settled)
            return

        if path.startswith("/payment/"):
            payment_id = path[len("/payment/") :]
            with LOCK:
                payment = PAYMENTS.get(payment_id)
                scripted = STATUSES.get(payment_id)
            if payment is None:
                # What the bridge answers for a payment it never minted, and what the server
                # turns into `UnknownOrder` rather than a retry loop.
                self._send(404, {"error": f"no payment {payment_id}"})
                return
            self._send(200, self._status_object(payment, scripted))
            return

        self._send(404, {"error": f"unknown path {path}"})

    def do_POST(self):  # noqa: N802
        global MINTED

        parsed = urlparse(self.path)
        path = parsed.path
        query = {key: values[0] for key, values in parse_qs(parsed.query).items()}
        raw = self._read_body()

        try:
            body = json.loads(raw) if raw else {}
        except json.JSONDecodeError:
            self._send(400, {"error": "body is not JSON"})
            return

        self._record("POST", path, query, body)

        if path == "/_control/status":
            self._script_status(body)
            return

        if path == "/_control/transactions":
            # **Appended, not replaced.** The settled list is the stub's own ledger now, so
            # what the run adds here is the settlements this stub did not mint: an amount the
            # server holds a row for, and one no row of ours carries.
            with LOCK:
                EXTRA_TRANSACTIONS.extend(body.get("transactions", []))
            self._send(200, {"ok": True})
            return

        if path == "/_control/reset":
            # The *scripted* state only. The minted payments are the stub's ledger of what it
            # was asked, and forgetting them would be forgetting the run's own requests.
            with LOCK:
                STATUSES.clear()
                EXTRA_TRANSACTIONS.clear()
            self._send(200, {"ok": True})
            return

        if path == "/payments":
            # A missing field is a 400 rather than a default: an invoice the server cannot
            # read is the failure this whole contract exists to prevent, and a defaulted
            # amount would make the smoke run's amount assertion vacuous.
            for field in ("amount", "reference", "minutes"):
                if field not in body:
                    self._send(400, {"error": f"the invoice request has no {field!r}"})
                    return

            with LOCK:
                MINTED += 1
                payment_id = f"stub-{MINTED}"
                amount = int(body["amount"])
                minutes = int(body["minutes"])
                payable = unique_amount(amount, MINTED)
                created = datetime.now(timezone.utc)
                invoice = {
                    "id": payment_id,
                    "status": "pending",
                    "amount": amount,
                    "uniqueAmount": payable,
                    "qrString": f"STUBQRIS:{body['reference']}:{payable}",
                    "reference": body["reference"],
                    "createdAt": rfc3339(created),
                    "expiresAt": rfc3339(created + timedelta(minutes=minutes)),
                    "secondsLeft": minutes * 60,
                    # The settlement, once the run scripts one. Never serialised: the bridge
                    # answers its invoice object, and a settlement is a `transaction`.
                    "settled": None,
                }
                PAYMENTS[payment_id] = invoice

            self._send(200, self._status_object(invoice, None))
            return

        self._send(404, {"error": f"unknown path {path}"})

    def _script_status(self, body: dict) -> None:
        """Script the answer `GET /payment/<id>` gives, keyed by **the gateway's own id**.

        The id is the bridge's, never our order id: the server asks with the id it was handed
        at mint time (`mapping::status_path`), so a control surface keyed by anything else
        would be scripting an answer nobody reads.

        The status word is **not validated here**. It is echoed verbatim, so a word the bridge
        never sends surfaces as the server's own `Invalid` — the failure a typo should produce
        — rather than as a stub that silently answers something the server accepts.
        """
        payment_id = body.get("payment_id")
        status = body.get("status")
        if not isinstance(payment_id, str) or not isinstance(status, str):
            self._send(400, {"error": "the scripted answer needs 'payment_id' and 'status'"})
            return

        with LOCK:
            payment = PAYMENTS.get(payment_id)
            if payment is None:
                self._send(404, {"error": f"no payment {payment_id}"})
                return

            scripted = {"status": status, "paid_at": body.get("paid_at")}
            if "uniqueAmount" in body:
                scripted["uniqueAmount"] = int(body["uniqueAmount"])

            if status == "paid":
                # The settlement instant. The bridge attaches one in the same transition that
                # marks a payment paid and the server refuses a paid answer without it, so a
                # script that omits one gets this process's clock: a missing instant is the
                # script's omission, not a fact about a payment.
                scripted["paid_at"] = body.get("paid_at") or rfc3339(datetime.now(timezone.utc))
                payment["status"] = "paid"
                if payment["settled"] is None:
                    # The first settlement is the one that landed. A re-scripted answer is the
                    # run asking the same question again, not a second payment.
                    payment["settled"] = {"time": scripted["paid_at"]}
            elif status in ("expired", "cancelled"):
                payment["status"] = status
                payment["settled"] = None
            # Any other word is echoed and **not** reflected in this stub's own ledger: the run
            # is driving a mistake, and `GET /transactions` must not report a settlement that a
            # paid word never produced.

            STATUSES[payment_id] = scripted

        self._send(200, {"ok": True})


def main() -> int:
    global LOG_PATH

    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, default=8200)
    parser.add_argument("--log", default=None, help="JSONL of everything the stub was asked")
    args = parser.parse_args()

    LOG_PATH = args.log

    server = ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    print(f"stub QRIS bridge on http://127.0.0.1:{args.port}", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
