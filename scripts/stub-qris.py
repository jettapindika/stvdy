#!/usr/bin/env python3
"""A stub QRIS gateway, for the Phase 13 smoke run and nothing else.

**B1 is unresolved**: the real gateway has no identity, no docs, no response shapes and
no fee rates, so the honest smoke run exercises the *wire the server sends* against this
and records which gateway it used. The server's side of that wire is
`crates/server/src/integrations/payment/mapping.rs`, the one file B1 changes — so this
stub speaks exactly the internal contract `mapping.rs` defines and asserts nothing about
the vendor. When the docs arrive, `mapping.rs` changes and this file changes with it;
no behavioural test in `crates/server/tests/` does, because those run against the trait's
`MockPayment`.

Three endpoints, the three the client calls:

  POST /invoices                  -> an invoice, derived from the request
  GET  /invoices/<order_id>       -> the order's status (scripted, `pending` by default)
  GET  /transactions?since_hours= -> the recent-transaction list (scripted, empty by default)

And two control endpoints the smoke script drives:

  POST /_control/status           -> script the next status `get_status` will report
  GET  /_control/requests         -> everything this stub was asked, in order

Two properties are deliberate:

  * **The invoice is derived from the request**, not canned. `qr_string` is
    `STUBQRIS:<order_id>:<amount_idr>` and `expires_at` is echoed from the request, so a
    smoke run can assert that the code a student scans is the gateway's own string and
    that the expiry on the row is the one we sent — a canned invoice would let both be
    wrong without the run noticing.
  * **The auth header is checked.** `--api-key` must match the `Authorization: Bearer`
    header the server sends, and a mismatch is a 401. That is the only way a smoke run
    can see the header at all, and it is what makes `mapping::auth_header` observable on
    the wire rather than a claim in a doc comment.
"""

import argparse
import json
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse

# Everything the stub has been asked, in order: `{"method", "path", "auth", "body"}`.
REQUESTS: list[dict] = []
# `order_id -> the status object `GET /invoices/<id>` answers with`.
STATUSES: dict[str, dict] = {}
# The list `GET /transactions` answers with.
TRANSACTIONS: list[dict] = []
LOCK = threading.Lock()

LOG_PATH: str | None = None
API_KEY: str | None = None


def record(entry: dict) -> None:
    with LOCK:
        REQUESTS.append(entry)
    if LOG_PATH:
        with open(LOG_PATH, "a", encoding="utf-8") as handle:
            handle.write(json.dumps(entry) + "\n")


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *args):  # noqa: ARG002
        """Quiet: the smoke script reports what matters, not the access log."""

    def _send(self, status: int, payload: dict) -> None:
        body = json.dumps(payload).encode("utf-8")
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _read_body(self) -> bytes:
        length = int(self.headers.get("content-length") or 0)
        return self.rfile.read(length) if length else b""

    def _authorised(self) -> bool:
        if API_KEY is None:
            return True
        header = self.headers.get("authorization") or ""
        return header == f"Bearer {API_KEY}"

    def do_GET(self):  # noqa: N802
        parsed = urlparse(self.path)
        path = parsed.path

        if path == "/_control/requests":
            with LOCK:
                self._send(200, {"requests": list(REQUESTS)})
            return

        if not self._authorised():
            record({"method": "GET", "path": path, "auth": None, "body": None})
            self._send(401, {"error": "bad or missing API key"})
            return

        if path == "/transactions":
            params = parse_qs(parsed.query)
            since = params.get("since_hours", [None])[0]
            record({"method": "GET", "path": path, "since_hours": since})
            with LOCK:
                self._send(200, {"transactions": list(TRANSACTIONS)})
            return

        if path.startswith("/invoices/"):
            order_id = path[len("/invoices/") :]
            record({"method": "GET", "path": path})
            with LOCK:
                status = STATUSES.get(order_id, {"status": "pending"})
            self._send(200, dict(status))
            return

        record({"method": "GET", "path": path})
        self._send(404, {"error": f"unknown path {path}"})

    def do_POST(self):  # noqa: N802
        parsed = urlparse(self.path)
        path = parsed.path
        raw = self._read_body()

        try:
            body = json.loads(raw) if raw else {}
        except json.JSONDecodeError:
            self._send(400, {"error": "body is not JSON"})
            return

        if path == "/_control/status":
            order_id = body["order_id"]
            with LOCK:
                STATUSES[order_id] = body["status"]
            self._send(200, {"ok": True})
            return

        if path == "/_control/transactions":
            with LOCK:
                TRANSACTIONS.clear()
                TRANSACTIONS.extend(body.get("transactions", []))
            self._send(200, {"ok": True})
            return

        if path == "/_control/reset":
            with LOCK:
                STATUSES.clear()
                TRANSACTIONS.clear()
            self._send(200, {"ok": True})
            return

        if not self._authorised():
            record(
                {
                    "method": "POST",
                    "path": path,
                    "auth": self.headers.get("authorization"),
                    "body": body,
                }
            )
            self._send(401, {"error": "bad or missing API key"})
            return

        if path == "/invoices":
            record(
                {
                    "method": "POST",
                    "path": path,
                    "auth": self.headers.get("authorization"),
                    "body": body,
                }
            )
            # A missing field is a 400 rather than a default: an invoice the server cannot
            # read is the failure this whole contract exists to prevent, and a defaulted
            # amount would make the smoke run's amount assertion vacuous.
            for field in ("order_id", "amount_idr", "expires_at"):
                if field not in body:
                    self._send(400, {"error": f"the invoice request has no {field!r}"})
                    return
            self._send(
                200,
                {
                    "order_id": body["order_id"],
                    "qr_string": f"STUBQRIS:{body['order_id']}:{body['amount_idr']}",
                    "expires_at": body["expires_at"],
                    "provider_payment_id": f"stub-{body['order_id']}",
                },
            )
            return

        self._send(404, {"error": f"unknown path {path}"})


def main() -> int:
    global LOG_PATH, API_KEY

    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, default=8200)
    parser.add_argument("--api-key", default=None, help="required in the Authorization header")
    parser.add_argument("--log", default=None, help="JSONL of everything the stub was asked")
    args = parser.parse_args()

    LOG_PATH = args.log
    API_KEY = args.api_key

    server = ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    print(f"stub QRIS gateway on http://127.0.0.1:{args.port}", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
