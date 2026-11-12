#!/usr/bin/env python3
"""Deterministic stdio DAP adapter used by koren-debug integration tests."""
import json
import os
import sys

MODE = os.environ.get("KOREN_FAKE_DAP_MODE", "normal")

def read_message():
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        if line in (b"\r\n", b"\n"):
            break
        if line.lower().startswith(b"content-length:"):
            length = int(line.split(b":", 1)[1].strip())
    if length is None:
        raise ValueError("missing Content-Length")
    return json.loads(sys.stdin.buffer.read(length))


def send(message):
    payload = json.dumps(message, separators=(",", ":")).encode()
    sys.stdout.buffer.write(f"Content-Length: {len(payload)}\r\n\r\n".encode() + payload)
    sys.stdout.buffer.flush()


def response(request, body=None):
    send({"seq": 100, "type": "response", "request_seq": request["seq"],
          "command": request["command"], "success": True, "body": body or {}})


while True:
    request = read_message()
    if request is None:
        break
    command = request["command"]
    if command == "initialize":
        if MODE == "crash":
            raise SystemExit(23)
        if MODE == "failed-initialize":
            send({"seq": 100, "type": "response", "request_seq": request["seq"],
                  "command": command, "success": False, "message": "fixture init failure"})
            break
        if MODE == "out-of-order-response":
            send({"seq": 97, "type": "response", "request_seq": request["seq"] + 100,
                  "command": "threads", "success": True, "body": {}})
        if MODE == "reverse-request":
            send({"seq": 99, "type": "request", "command": "runInTerminal",
                  "arguments": {"kind": "integrated", "args": ["must-not-run"]}})
            reverse_response = read_message()
            if (reverse_response.get("type") != "response"
                    or reverse_response.get("success") is not False
                    or reverse_response.get("command") != "runInTerminal"):
                raise RuntimeError("unsupported reverse request was not rejected")
            send({"seq": 98, "type": "event", "event": "output",
                  "body": {"category": "console", "output": "reverse request rejected"}})
        response(request, {"supportsConfigurationDoneRequest": True,
                            "supportsStepInTargetsRequest": False,
                            "supportsVariablePaging": True,
                            "supportsTerminateRequest": True})
        send({"seq": 101, "type": "event", "event": "initialized", "body": {}})
        if MODE == "disconnect-after-init":
            break
    elif command == "launch" or command == "attach":
        response(request)
        send({"seq": 102, "type": "event", "event": "stopped",
              "body": {"reason": "breakpoint", "threadId": 1, "allThreadsStopped": True}})
    elif command == "setBreakpoints":
        bps = request.get("arguments", {}).get("breakpoints", [])
        response(request, {"breakpoints": [{"verified": True, "line": bp["line"]} for bp in bps]})
    elif command == "threads":
        response(request, {"threads": [{"id": 1, "name": "fake-main"}]})
    elif command == "stackTrace":
        response(request, {"stackFrames": [{"id": 7, "name": "main", "line": 4,
                                              "column": 1, "source": {"name": "main.src",
                                              "path": "/tmp/main.src"}}], "totalFrames": 1})
    elif command == "scopes":
        response(request, {"scopes": [{"name": "Locals", "variablesReference": 11,
                                        "expensive": False}]})
    elif command == "variables":
        response(request, {"variables": [{"name": "answer", "value": "42", "type": "int",
                                           "variablesReference": 0}]})
    elif command == "configurationDone":
        response(request)
    elif command in ("continue", "next", "stepIn", "stepOut", "pause"):
        response(request, {"allThreadsContinued": True})
        if command == "continue":
            send({"seq": 103, "type": "event", "event": "continued",
                  "body": {"threadId": 1, "allThreadsContinued": True}})
    elif command == "disconnect" or command == "terminate":
        response(request)
        if command == "disconnect":
            send({"seq": 104, "type": "event", "event": "terminated", "body": {}})
            break
    else:
        response(request, {"message": "unsupported fake request"})
