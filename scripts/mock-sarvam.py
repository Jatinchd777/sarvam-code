#!/usr/bin/env python3
"""mock-sarvam.py — a toy OpenAI-compatible endpoint for testing the harness offline.

Returns a scripted 3-request conversation:
  1. tool_call: read_file {"path": "README.md"}
  2. tool_call: bash {"command": "echo hello-from-bash"}
  3. final text answer

Every incoming request body is appended to $MOCK_LOG as JSONL, so you can inspect
exactly what the harness sent back (assistant tool_calls, tool results, ids).

Usage:
    python3 scripts/mock-sarvam.py
    SARVAM_API_KEY=dummy \
    SARVAM_API_URL=http://127.0.0.1:8877/v2/chat/completions \
    PERMISSION_MODE=read-only ./scripts/sarvam-loop.sh "test the loop"

Env:
    MOCK_PORT  default 8877
    MOCK_LOG   default /tmp/mock-sarvam-requests.jsonl
"""
import json
import os
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

LOG = os.environ.get("MOCK_LOG", "/tmp/mock-sarvam-requests.jsonl")
PORT = int(os.environ.get("MOCK_PORT", "8877"))
state = {"n": 0}


def scripted_response(model: str, n: int) -> dict:
    if n == 1:
        message = {
            "role": "assistant",
            "content": None,
            "tool_calls": [{
                "id": "call_read",
                "type": "function",
                "function": {
                    "name": "read_file",
                    "arguments": json.dumps({"path": "README.md"}),
                },
            }],
        }
        finish = "tool_calls"
    elif n == 2:
        message = {
            "role": "assistant",
            "content": None,
            "tool_calls": [{
                "id": "call_bash",
                "type": "function",
                "function": {
                    "name": "bash",
                    "arguments": json.dumps({"command": "echo hello-from-bash"}),
                },
            }],
        }
        finish = "tool_calls"
    else:
        message = {
            "role": "assistant",
            "content": "Mock done. I read README.md and attempted bash.",
        }
        finish = "stop"

    return {
        "id": "chatcmpl-mock",
        "object": "chat.completion",
        "created": 0,
        "model": model,
        "choices": [{"index": 0, "message": message, "finish_reason": finish}],
        "usage": {"prompt_tokens": 10, "completion_tokens": 10, "total_tokens": 20},
    }


class Handler(BaseHTTPRequestHandler):
    def do_POST(self):
        raw = self.rfile.read(int(self.headers.get("Content-Length", 0)))
        request = json.loads(raw)

        state["n"] += 1
        with open(LOG, "a", encoding="utf-8") as f:
            f.write(json.dumps(request) + "\n")

        payload = json.dumps(scripted_response(request.get("model", "mock"), state["n"]))
        data = payload.encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *args):
        pass


if __name__ == "__main__":
    print(
        f"mock listening on http://127.0.0.1:{PORT}/v2/chat/completions\n"
        f"request log: {LOG}",
        flush=True,
    )
    ThreadingHTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
