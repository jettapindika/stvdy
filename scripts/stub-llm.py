#!/usr/bin/env python3
"""A stub OpenAI-compatible model server for the Phase 2 smoke run.

Exists because the trait-level double used by the Rust suite cannot catch a wire
format mistake: it stands in for the provider at the boundary the server *thinks*
it is talking to. This one speaks real HTTP and real SSE, so the smoke run
exercises the actual request body and the actual frame parsing.

Two modes:
  * default: stream a fixed answer that cites [1] and [9], where [9] was never
    sent. The server must drop it. That is the citation-hallucination gate.
  * the answer text is configurable with --answer, and --broken makes the first
    token arrive and then the connection die mid-stream.

A run that needs *different* answers per request (Phase 6's report outline, then a
report section, then a cheatsheet) passes --answers-file with a JSON array. The
answers are served in order and the last one repeats, so the count of calls the
server makes is not something the stub has to be told in advance.
"""

import argparse
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    answer = "Mitochondria produce ATP through oxidative phosphorylation [1], and also [9]."
    # The answers to serve, in order, one per request. The last one repeats once the
    # list is exhausted, so a run that makes one more call than it planned does not
    # fail with "no queued response left" and look like a server bug.
    answers = None
    # How many requests have been served, so the answer above advances.
    served = 0
    broken = False
    # Every request body, appended as one JSON object per line. The smoke run reads
    # this to assert on the prompt the model actually received, which a
    # response-only check cannot see.
    log_path = None

    def log_message(self, *args):  # noqa: ARG002
        """Quiet: the smoke script reports what matters, not the HTTP access log."""

    def current_answer(self):
        """The answer for this request: the next one in the list, or the fixed one."""
        if Handler.answers is None:
            return Handler.answer
        index = min(Handler.served, len(Handler.answers) - 1)
        return Handler.answers[index]

    def do_POST(self):
        length = int(self.headers.get("content-length", "0"))
        raw = self.rfile.read(length)
        try:
            body = json.loads(raw)
        except json.JSONDecodeError:
            self.send_error(400, "not json")
            return

        if Handler.log_path:
            with open(Handler.log_path, "a", encoding="utf-8") as handle:
                handle.write(json.dumps(body) + "\n")

        if not self.path.endswith("/chat/completions"):
            self.send_error(404, "unknown path")
            return
        if not self.headers.get("authorization", "").startswith("Bearer "):
            self.send_error(401, "no bearer")
            return

        Handler.served += 1

        if not body.get("stream"):
            payload = json.dumps(
                {
                    "choices": [{"message": {"content": self.current_answer()}}],
                    "usage": {"prompt_tokens": 100, "completion_tokens": 20, "cost": 0.00042},
                }
            ).encode()
            self.send_response(200)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)
            return

        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("cache-control", "no-cache")
        self.send_header("connection", "close")
        self.end_headers()

        # Word by word, so the client has to concatenate rather than receive one
        # token: a server that buffered the whole answer would still pass a
        # single-token test.
        words = self.current_answer().split(" ")
        for index, word in enumerate(words):
            text = word if index == 0 else " " + word
            frame = {"choices": [{"delta": {"content": text}}]}
            self.wfile.write(f"data: {json.dumps(frame)}\n\n".encode())
            self.wfile.flush()

        if Handler.broken:
            # A connection that dies mid-stream: the client must keep what it got.
            self.wfile.write(b"data: {\"choices\":[{\"delta\":{\"content\":\" trunc")
            self.wfile.flush()
            self.close_connection = True
            return

        usage = {
            "choices": [],
            "usage": {"prompt_tokens": 812, "completion_tokens": 57, "cost": 0.00119},
        }
        self.wfile.write(f"data: {json.dumps(usage)}\n\n".encode())
        self.wfile.write(b"data: [DONE]\n\n")
        self.wfile.flush()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, default=8100)
    parser.add_argument("--answer", default=Handler.answer)
    parser.add_argument(
        "--answers-file",
        default="",
        help="a JSON array of answers to serve in order; the last one repeats",
    )
    parser.add_argument("--broken", action="store_true")
    parser.add_argument("--log", default="", help="append every request body as JSONL")
    args = parser.parse_args()

    Handler.answer = args.answer
    if args.answers_file:
        with open(args.answers_file, encoding="utf-8") as handle:
            Handler.answers = json.load(handle)
        if not Handler.answers:
            parser.error("--answers-file must hold at least one answer")
    Handler.broken = args.broken
    Handler.log_path = args.log or None

    server = ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    print(f"stub llm listening on 127.0.0.1:{args.port}", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        sys.exit(0)


if __name__ == "__main__":
    main()
