"""Small stdlib-only LSP fixture used by the Rust transport tests."""

import json
import sys


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
    if "id" in message:
        result = None
        if method == "initialize":
            result = {"capabilities": {"hoverProvider": True, "completionProvider": {}, "definitionProvider": True}}
        elif method == "textDocument/hover":
            result = {"contents": {"kind": "markdown", "value": "hover info"}}
        elif method == "textDocument/completion":
            result = {"isIncomplete": False, "items": [{"label": "complete"}]}
        elif method == "textDocument/definition":
            result = []
        write_message({"jsonrpc": "2.0", "id": message["id"], "result": result})
    if method == "textDocument/didOpen":
        document = message["params"]["textDocument"]
        write_message({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": {
            "uri": document["uri"], "version": document["version"], "diagnostics": []
        }})
    if method == "exit":
        break
