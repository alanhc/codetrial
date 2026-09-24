#!/usr/bin/env python3
"""Answer Gemini `generateContent` calls from a local llama.cpp server.

CodeTrial's report and interim-review calls speak Gemini's REST API. This shim
accepts that envelope, forwards it to llama-server's OpenAI-compatible
`/v1/chat/completions`, and answers in Gemini's shape, so the Rust side only
needs `CODETRIAL_GEMINI_REST_BASE` pointed here. The live interviewer socket is
not handled; it still goes to Google.

    llama-server -m model.gguf --port 8080 -ngl 99 -c 32768
    scripts/gemini-shim.py --listen 127.0.0.1:8090 --llama http://127.0.0.1:8080
    CODETRIAL_GEMINI_REST_BASE=http://127.0.0.1:8090 make web

Standard library only, so it runs wherever the test gate's Python does.
"""

import argparse
import json
import re
import sys
import time
import urllib.error
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ROUTE = re.compile(r"^/v1beta/models/([^/:]+):generateContent$")

# A full report at 16k output tokens on a mid-size model can take a while; the
# Rust side enforces its own per-attempt deadline, so this only has to outlast it.
UPSTREAM_TIMEOUT_S = 300


def convert_schema(node):
    """Gemini's OpenAPI subset to the JSON Schema llama.cpp compiles to a grammar.

    Types are upper-case enum names there and lower-case here, `nullable`
    becomes an `anyOf` with null, and `propertyOrdering` becomes the order of
    `properties`, which is the order the grammar emits keys in. Objects are
    closed, so the model cannot pad a report with fields nobody reads.
    """
    if isinstance(node, list):
        return [convert_schema(item) for item in node]
    if not isinstance(node, dict):
        return node
    out = {}
    for key, value in node.items():
        if key in ("propertyOrdering", "nullable"):
            continue
        if key == "type" and isinstance(value, str):
            out["type"] = value.lower()
        elif key == "properties":
            order = node.get("propertyOrdering") or []
            names = [n for n in order if n in value] + [
                n for n in value if n not in order
            ]
            out["properties"] = {n: convert_schema(value[n]) for n in names}
        else:
            out[key] = convert_schema(value)
    if out.get("type") == "object":
        out.setdefault("additionalProperties", False)
    if node.get("nullable"):
        return {"anyOf": [out, {"type": "null"}]}
    return out


def parts_text(content):
    return "".join(part.get("text", "") for part in content.get("parts", []))


def to_chat_request(body):
    messages = []
    system = body.get("systemInstruction")
    if system:
        messages.append({"role": "system", "content": parts_text(system)})
    for content in body.get("contents", []):
        role = "assistant" if content.get("role") == "model" else "user"
        messages.append({"role": role, "content": parts_text(content)})

    config = body.get("generationConfig", {})
    request = {"messages": messages, "stream": False}
    if "temperature" in config:
        request["temperature"] = config["temperature"]
    if "maxOutputTokens" in config:
        request["max_tokens"] = config["maxOutputTokens"]
    if (
        config.get("responseMimeType") == "application/json"
        and "responseSchema" in config
    ):
        request["response_format"] = {
            "type": "json_schema",
            "json_schema": {
                "name": "response",
                "strict": True,
                "schema": convert_schema(config["responseSchema"]),
            },
        }

    # The Rust side asks for no thinking because Gemini charges thinking tokens
    # against maxOutputTokens; a reasoning model here does the same, so honour it.
    thinking = config.get("thinkingConfig", {})
    if thinking.get("thinkingBudget") == 0 or thinking.get("thinkingLevel") == "NONE":
        request["chat_template_kwargs"] = {"enable_thinking": False}
    return request


FINISH_REASONS = {"stop": "STOP", "length": "MAX_TOKENS"}


def to_gemini_response(chat):
    choice = (chat.get("choices") or [{}])[0]
    text = (choice.get("message") or {}).get("content") or ""
    usage = chat.get("usage") or {}
    return {
        "candidates": [
            {
                "content": {"role": "model", "parts": [{"text": text}]},
                "finishReason": FINISH_REASONS.get(
                    choice.get("finish_reason"), "OTHER"
                ),
            }
        ],
        "usageMetadata": {
            "promptTokenCount": usage.get("prompt_tokens", 0),
            "candidatesTokenCount": usage.get("completion_tokens", 0),
            "totalTokenCount": usage.get("total_tokens", 0),
        },
        "modelVersion": chat.get("model", "local"),
    }


class Handler(BaseHTTPRequestHandler):
    llama = "http://127.0.0.1:8080"

    def do_POST(self):
        match = ROUTE.match(self.path.split("?", 1)[0])
        if not match:
            self.reply(404, error_body(404, f"no route for {self.path}"))
            return
        try:
            length = int(self.headers.get("Content-Length", "0"))
            body = json.loads(self.rfile.read(length) or b"{}")
        except (ValueError, json.JSONDecodeError) as error:
            self.reply(400, error_body(400, f"bad request body: {error}"))
            return

        request = to_chat_request(body)
        started = time.monotonic()
        upstream = urllib.request.Request(
            f"{self.llama}/v1/chat/completions",
            data=json.dumps(request).encode(),
            headers={"Content-Type": "application/json"},
        )
        try:
            with urllib.request.urlopen(
                upstream, timeout=UPSTREAM_TIMEOUT_S
            ) as response:
                chat = json.load(response)
        except urllib.error.HTTPError as error:
            # Status passes through so the Rust retry rules see a 503 as a 503.
            detail = error.read().decode(errors="replace")[:500]
            self.log_message("upstream %d: %s", error.code, detail)
            self.reply(error.code, error_body(error.code, detail))
            return
        except (urllib.error.URLError, TimeoutError, ConnectionError) as error:
            self.log_message("upstream unreachable: %s", error)
            self.reply(503, error_body(503, f"llama-server unreachable: {error}"))
            return

        answer = to_gemini_response(chat)
        meta = answer["usageMetadata"]
        self.log_message(
            "%s -> %s: %d in, %d out, %s, %.1fs",
            match.group(1),
            answer["modelVersion"],
            meta["promptTokenCount"],
            meta["candidatesTokenCount"],
            answer["candidates"][0]["finishReason"],
            time.monotonic() - started,
        )
        self.reply(200, answer)

    def reply(self, status, payload):
        data = json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, format, *args):
        sys.stderr.write(f"[gemini-shim] {format % args}\n")


def error_body(code, message):
    return {
        "error": {
            "code": code,
            "message": message,
            "status": "UNAVAILABLE" if code == 503 else "ERROR",
        }
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--listen", default="127.0.0.1:8090", help="host:port to serve on"
    )
    parser.add_argument(
        "--llama", default="http://127.0.0.1:8080", help="llama-server base URL"
    )
    args = parser.parse_args()

    host, _, port = args.listen.rpartition(":")
    Handler.llama = args.llama.rstrip("/")
    server = ThreadingHTTPServer((host or "127.0.0.1", int(port)), Handler)
    sys.stderr.write(f"[gemini-shim] {args.listen} -> {Handler.llama}\n")
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
