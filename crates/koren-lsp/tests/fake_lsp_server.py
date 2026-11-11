"""Small stdlib-only LSP fixture used by the Rust transport tests."""

import json
import os
import pathlib
import sys
import time


def wait_for_gate(name, reached_name):
    """Block deterministically until the named gate file appears, when configured."""
    gate = os.environ.get(name)
    if not gate:
        return
    reached = os.environ.get(reached_name)
    if reached:
        pathlib.Path(reached).touch()
    deadline = time.monotonic() + 15
    while not os.path.exists(gate):
        if time.monotonic() >= deadline:
            return
        time.sleep(0.005)


def trace(message):
    destination = os.environ.get("FAKE_LSP_TRACE")
    if destination:
        with open(destination, "a", encoding="utf-8") as output:
            output.write(json.dumps(message, separators=(",", ":")) + "\n")


def read_message():
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        if line in (b"\r\n", b"\n"):
            break
        name, _, value = line.partition(b":")
        if name.lower() == b"content-length":
            length = int(value.strip())
    if length is None or length > 16 * 1024 * 1024:
        raise RuntimeError("invalid Content-Length")
    return json.loads(sys.stdin.buffer.read(length))


def write_message(message):
    body = json.dumps(message, separators=(",", ":")).encode()
    sys.stdout.buffer.write(f"Content-Length: {len(body)}\r\n\r\n".encode() + body)
    sys.stdout.buffer.flush()


while True:
    message = read_message()
    if message is None:
        break
    method = message.get("method", "")
    if method == "$/cancelRequest":
        cancelled = os.environ.get("FAKE_LSP_CANCEL_REACHED")
        if cancelled:
            pathlib.Path(cancelled).touch()
        trace(message)
    if "id" in message:
        result = None
        if method == "initialize":
            wait_for_gate("FAKE_LSP_INIT_GATE", "FAKE_LSP_INIT_REACHED")
            delay = int(os.environ.get("FAKE_LSP_INIT_DELAY_MS", "0"))
            if delay:
                time.sleep(delay / 1000)
            if os.environ.get("FAKE_LSP_CRASH_ON_INIT") == "1":
                os._exit(31)
            result = {"capabilities": {"hoverProvider": True, "completionProvider": {}, "definitionProvider": True}}
        elif method == "textDocument/hover":
            wait_for_gate("FAKE_LSP_HOVER_GATE", "FAKE_LSP_HOVER_REACHED")
            if os.environ.get("FAKE_LSP_CRASH_ON_HOVER") == "1":
                os._exit(32)
            result = {"contents": {"kind": "markdown", "value": "hover info"}}
        elif method == "textDocument/completion":
            wait_for_gate("FAKE_LSP_COMPLETION_GATE", "FAKE_LSP_COMPLETION_REACHED")
            result = {"isIncomplete": False, "items": [{"label": "complete"}]}
        elif method == "textDocument/definition":
            result = []
        write_message({"jsonrpc": "2.0", "id": message["id"], "result": result})
    if method == "textDocument/didOpen":
        document = message["params"]["textDocument"]
        trace(message)
        write_message({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": {
            "uri": document["uri"], "version": document["version"], "diagnostics": []
        }})
    if method in ("textDocument/didChange", "textDocument/didClose"):
        trace(message)
    if method == "textDocument/didChange":
        document = message["params"]["textDocument"]
        write_message({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": {
            "uri": document["uri"], "version": document["version"], "diagnostics": []
        }})
    if method == "exit":
        break
