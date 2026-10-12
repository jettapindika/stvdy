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
report section, then a cheatsheet) passes --answers-file with a JSON array of
`{"contains": "<a string the prompt carries>", "answer": "<the reply>"}`. The first
entry whose marker appears in the prompt is served; an entry without a marker is the
fallback. Matching on the prompt rather than on the request's position is what makes
the file reusable: a positional list answers correctly once and every later run of the
same script gets the answers shifted by one.

A third mode, --tool-calls, is Phase 12's: the reply is a streamed *tool call* rather
than prose, so the smoke run can drive the server's gate and the confirmation card
through a real wire format. The file is a JSON object with a `tool_calls` array of
`{"contains"?, "text", "calls": [{"id","name","arguments"}]}`, matched on the prompt
the same way. **The tool schema the server sent is validated here**, because that is
the mistake a trait-level double cannot see: a request whose `tools` array is missing
or whose entries carry no `parameters` object is a 400, which turns a wire-format bug
into a visible failure at the stub rather than a confusing refusal from the model.
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
    # Phase 12's mode: a tool-call script instead of prose. `None` keeps the prose modes.
    tool_calls = None

    def log_message(self, *args):  # noqa: ARG002
        """Quiet: the smoke script reports what matters, not the HTTP access log."""

    def current_answer(self, prompt):
        """The answer for this request.

        With `--answers-file`, each entry is `{"contains": "...", "answer": "..."}` and the
        first entry whose `contains` appears in the prompt wins; an entry with no `contains`
        is the fallback. Matching on the *prompt* rather than on the request's position is
        deliberate: a positional list only answers correctly once, so a second run of the
        same smoke script — which is the normal case, and the only way to debug a failure —
        would get the answers shifted by one and report a parse error that looks like a
        server bug. `contains` is a string the module's own prompt text carries, so it is
        stable across a re-run and across a change to the script's order of calls.
        """
        if Handler.answers is None:
            return Handler.answer
        fallback = Handler.answer
        for entry in Handler.answers:
            if isinstance(entry, str):
                fallback = entry
                continue
            marker = entry.get("contains")
            if marker is None:
                fallback = entry.get("answer", fallback)
            elif marker in prompt:
                return entry.get("answer", fallback)
        return fallback

    def current_tool_call(self, prompt):
        """The scripted tool call for this request, or None in the prose modes.

        Matched on the prompt for the same reason `--answers-file` is: a positional
        list answers correctly once, and a re-run — which is how a failure is debugged —
        would get the calls shifted by one.
        """
        if Handler.tool_calls is None:
            return None
        fallback = None
        for entry in Handler.tool_calls:
            marker = entry.get("contains")
            if marker is None:
                fallback = entry
            elif marker in prompt:
                return entry
        return fallback

    def validate_tools(self, body):
        """Refuses a request whose tool schema is malformed.

        The one thing this mode exists for: a `tools` array that is absent when the
        plan grants writes, or an entry with no `parameters` object, is a wire-format
        mistake the trait-level double cannot see. Answering 400 makes it a visible
        failure here rather than a refusal the model would have to phrase.
        """
        tools = body.get("tools")
        if not tools:
            return None
        for tool in tools:
            if not isinstance(tool, dict) or "name" not in tool:
                return "a tool entry carries no name"
            if not isinstance(tool.get("parameters"), dict):
                return f"{tool['name']} carries no parameters object"
        return None

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

        prompt = body.get("messages", [{}])[0].get("content", "")

        if Handler.tool_calls is not None:
            problem = self.validate_tools(body)
            if problem is not None:
                self.send_error(400, problem)
                return

        answer = self.current_answer(prompt)
        Handler.served += 1

        if not body.get("stream"):
            payload = json.dumps(
                {
                    "choices": [{"message": {"content": answer}}],
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

        scripted = self.current_tool_call(prompt)
        if scripted is not None:
            # The prose first, then the calls, then the usage frame — which is the order a
            # real provider streams them in, and the order the server's gate has to handle:
            # a proposal arriving mid-answer must not interrupt the text.
            for index, word in enumerate(scripted.get("text", "").split(" ")):
                text = word if index == 0 else " " + word
                frame = {"choices": [{"delta": {"content": text}}]}
                self.wfile.write(f"data: {json.dumps(frame)}\n\n".encode())
                self.wfile.flush()
            for call in scripted.get("calls", []):
                frame = {
                    "choices": [
                        {
                            "delta": {
                                "tool_calls": [
                                    {
                                        "index": 0,
                                        "id": call["id"],
                                        "type": "function",
                                        "function": {
                                            "name": call["name"],
                                            "arguments": json.dumps(call["arguments"]),
                                        },
                                    }
                                ]
                            }
                        }
                    ]
                }
                self.wfile.write(f"data: {json.dumps(frame)}\n\n".encode())
                self.wfile.flush()
            usage = {
                "choices": [],
                "usage": {"prompt_tokens": 812, "completion_tokens": 57, "cost": 0.00119},
            }
            self.wfile.write(f"data: {json.dumps(usage)}\n\n".encode())
            self.wfile.write(b"data: [DONE]\n\n")
            self.wfile.flush()
            return

        # Word by word, so the client has to concatenate rather than receive one
        # token: a server that buffered the whole answer would still pass a
        # single-token test.
        words = answer.split(" ")
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
        help='a JSON array of {"contains": "...", "answer": "..."}; first match wins',
    )
    parser.add_argument(
        "--tool-calls",
        default="",
        help='a JSON file with {"tool_calls":[{"contains"?","text","calls":[…]}]}; streams tool calls',
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
    if args.tool_calls:
        with open(args.tool_calls, encoding="utf-8") as handle:
            script = json.load(handle)
        Handler.tool_calls = script.get("tool_calls") or []
        if not Handler.tool_calls:
            parser.error("--tool-calls must hold at least one entry")
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
