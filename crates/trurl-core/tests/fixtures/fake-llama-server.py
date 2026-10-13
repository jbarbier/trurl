#!/usr/bin/env python3
"""A stand-in for llama-server, for tests that need answers but not a model.

Takes the same command line as the real one (only --port and --api-key-file
matter), answers /health, and streams an OpenAI-style answer: a message that
contains SLOW is answered one word every 50 ms for 30 s (long enough to be
interrupted), anything else in three quick words.
"""

import json
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

args = sys.argv[1:]
port = int(args[args.index("--port") + 1])
key = open(args[args.index("--api-key-file") + 1]).read().strip()


class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.0"

    def log_message(self, *a):
        pass

    def do_GET(self):
        body = b'{"status":"ok"}' if self.path == "/health" else b"{}"
        self.send_response(200 if self.path == "/health" else 404)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        if self.headers.get("Authorization") != f"Bearer {key}":
            self.send_response(401)
            self.end_headers()
            return
        req = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        last = req["messages"][-1]["content"]
        words = ["word "] * 600 if "SLOW" in last else ["Hello ", "from ", "test."]
        delay = 0.05 if "SLOW" in last else 0.0
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        try:
            for w in words:
                chunk = {"choices": [{"index": 0, "finish_reason": None, "delta": {"content": w}}]}
                self.wfile.write(f"data: {json.dumps(chunk)}\n\n".encode())
                self.wfile.flush()
                time.sleep(delay)
            end = {"choices": [{"index": 0, "finish_reason": "stop", "delta": {}}]}
            self.wfile.write(f"data: {json.dumps(end)}\n\ndata: [DONE]\n\n".encode())
        except (BrokenPipeError, ConnectionResetError):
            pass


ThreadingHTTPServer(("127.0.0.1", port), H).serve_forever()
