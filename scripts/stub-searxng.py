#!/usr/bin/env python3
"""A stub SearXNG for the Phase 3 smoke run.

Exists because this machine cannot run the real service: Docker is not running
here, and A3 measured that a laptop's engine list is a different thing entirely
from the production host's (1 of 8 engines answering versus 3). Tuning or
verifying the engine list from here would be tuning it against the wrong sample,
so the smoke run exercises the *wire format* against this and the real engine list
is verified on the staging VPS.

Speaks the JSON API the server's tier actually calls:
  GET /search?q=<query>&format=json

The response deliberately includes `unresponsive_engines`, because the server
captures it and writes it to `usage_events`: a reply without it would not exercise
the path that makes silent degradation visible.
"""

import argparse
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse

# One hit with a DOI and one without, so a single smoke run proves the citation
# carries a DOI when there is one and does not invent one when there is not.
RESULTS = [
    {
        "url": "https://arxiv.org/abs/1706.03762",
        "title": "Attention Is All You Need",
        "content": "The dominant sequence transduction models are based on complex "
        "recurrent or convolutional neural networks.",
        "publishedDate": "2017-06-12T00:00:00",
        "score": 4.2,
        "doi": "10.48550/arXiv.1706.03762",
    },
    {
        "url": "https://example.org/blog/transformers",
        "title": "A blog post about transformers",
        "content": "A plain-language explanation with no identifier attached.",
        "publishedDate": None,
        "score": 1.1,
    },
]


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    queries: list[str] = []

    def log_message(self, *args):  # noqa: ARG002
        """Quiet: the smoke script reports what matters, not the access log."""

    def do_GET(self):
        parsed = urlparse(self.path)
        if not parsed.path.endswith("/search"):
            self.send_error(404, "unknown path")
            return

        params = parse_qs(parsed.query)
        if params.get("format", [""])[0] != "json":
            # The server always asks for json; a test that forgot would get SearXNG's
            # HTML page in production, which is a parse failure rather than an error.
            self.send_error(400, "format=json is required")
            return

        query = params.get("q", [""])[0]
        Handler.queries.append(query)

        body = json.dumps(
            {
                "query": query,
                "number_of_results": len(RESULTS),
                "results": RESULTS,
                # The engine-health list the server records. Two engines, one with a
                # reason, which is exactly the shape A3 describes.
                "unresponsive_engines": [["brave", "too many requests"], ["duckduckgo", "timeout"]],
            }
        ).encode()

        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, default=8088)
    parser.add_argument("--log", default="", help="append every query as JSONL")
    args = parser.parse_args()

    Handler.queries = []
    if args.log:
        import atexit

        def dump():
            with open(args.log, "w", encoding="utf-8") as handle:
                for query in Handler.queries:
                    handle.write(json.dumps({"q": query}) + "\n")

        atexit.register(dump)

    server = ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    print(f"stub searxng listening on 127.0.0.1:{args.port}", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        sys.exit(0)


if __name__ == "__main__":
    main()