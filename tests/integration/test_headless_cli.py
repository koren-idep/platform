import json
import pathlib
import subprocess

ROOT = pathlib.Path(__file__).resolve().parents[2]
BIN = ROOT / "target" / "debug" / "koren-headless"


def invoke(*args):
    return subprocess.run([str(BIN), *args], capture_output=True, text=True)


def test_version_and_command_list():
    version = invoke("--version")
    assert version.returncode == 0
    assert version.stdout.startswith("koren-headless ")
    commands = invoke("commands", "list", "--json")
    assert commands.returncode == 0
    assert json.loads(commands.stdout)["result"]["Ok"]["value"] == ["core.ping"]


def test_execute_ping():
    result = invoke("command", "execute", "core.ping")
    assert result.returncode == 0
    assert result.stdout.strip() == "pong"


def test_workspace_info_reports_entries(tmp_path):
    (tmp_path / "hello.txt").write_text("hello", encoding="utf-8")
    result = invoke("workspace", "info", str(tmp_path), "--json")
    assert result.returncode == 0, result.stderr
    response = json.loads(result.stdout)
    assert response["result"]["Ok"]["kind"] == "workspace"
    assert response["result"]["Ok"]["value"]["entries"] == 1


def test_invalid_workspace_has_nonzero_exit():
    result = invoke("workspace", "info", "/definitely/not/a/koren-workspace")
    assert result.returncode != 0
    assert "does not exist" in result.stderr


def test_json_lines_session_edits_and_saves_in_one_process(tmp_path):
    nested = tmp_path / "資料"
    nested.mkdir()
    source = nested / "source file.rs"
    source.write_text("hello\r\n", encoding="utf-8", newline="")
    requests = [
        {"version": 2, "request_id": 1, "method": {"method": "open_workspace", "params": {"path": str(tmp_path)}}},
        {"version": 2, "request_id": 2, "method": {"method": "list_directory", "params": {"path": "資料"}}},
        {"version": 2, "request_id": 3, "method": {"method": "open_document", "params": {"path": "資料/source file.rs"}}},
        {"version": 2, "request_id": 4, "method": {"method": "close_workspace"}},
        {"version": 2, "request_id": 5, "method": {"method": "read_document", "params": {"document_id": 1}}},
        {"version": 2, "request_id": 6, "method": {"method": "apply_document_edits", "params": {
            "document_id": 1,
            "expected_revision": 0,
            "edits": [{"range": {"start": {"line": 0, "character": 5}, "end": {"line": 0, "character": 5}}, "new_text": "!"}],
        }}},
        {"version": 2, "request_id": 7, "method": {"method": "save_document", "params": {"document_id": 1, "expected_revision": 1}}},
        {"version": 2, "request_id": 8, "method": {"method": "close_document", "params": {"document_id": 1, "discard_unsaved_changes": False}}},
        {"version": 2, "request_id": 9, "method": {"method": "close_workspace"}},
    ]
    payload = "".join(json.dumps(request, ensure_ascii=False) + "\n" for request in requests)
    result = subprocess.run([str(BIN), "session", "--stdio"], input=payload, capture_output=True, text=True)
    assert result.returncode == 0, result.stderr
    responses = [json.loads(line) for line in result.stdout.splitlines()]
    assert [response["request_id"] for response in responses] == list(range(1, 10))
    assert responses[2]["result"]["Ok"]["value"]["document_id"] == 1
    assert responses[3]["result"]["Err"]["code"] == "invalid_request"
    assert responses[4]["result"]["Ok"]["value"]["contents"] == "hello\r\n"
    assert responses[5]["result"]["Ok"]["value"] == {"document_id": 1, "revision": 1, "dirty": True}
    assert responses[6]["result"]["Ok"]["value"]["dirty"] is False
    assert source.read_bytes() == b"hello!\r\n"


def test_json_lines_session_conflict_is_structured_and_never_silent(tmp_path):
    source = tmp_path / "source.txt"
    source.write_text("original", encoding="utf-8")
    process = subprocess.Popen([str(BIN), "session", "--stdio"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)

    def request(request_id, method):
        process.stdin.write(json.dumps({"version": 2, "request_id": request_id, "method": method}) + "\n")
        process.stdin.flush()
        return json.loads(process.stdout.readline())

    assert "Ok" in request(1, {"method": "open_workspace", "params": {"path": str(tmp_path)}})["result"]
    assert request(2, {"method": "open_document", "params": {"path": "source.txt"}})["result"]["Ok"]["value"]["document_id"] == 1
    request(3, {"method": "apply_document_edits", "params": {"document_id": 1, "expected_revision": 0, "edits": [
        {"range": {"start": {"line": 0, "character": 8}, "end": {"line": 0, "character": 8}}, "new_text": " local"}
    ]}})
    source.write_text("external", encoding="utf-8")
    conflict = request(4, {"method": "save_document", "params": {"document_id": 1, "expected_revision": 1}})
    assert conflict["result"]["Err"]["code"] == "conflict"
    assert source.read_text(encoding="utf-8") == "external"
    resolved = request(5, {"method": "resolve_document_conflict", "params": {"document_id": 1, "expected_revision": 1, "decision": "overwrite"}})
    assert resolved["result"]["Ok"]["value"]["dirty"] is False
    assert source.read_text(encoding="utf-8") == "original local"
    request(6, {"method": "close_document", "params": {"document_id": 1, "discard_unsaved_changes": False}})
    request(7, {"method": "close_workspace"})
    process.stdin.close()
    assert process.wait(timeout=5) == 0
    assert process.stderr.read() == ""


def test_session_rejects_unsupported_protocol_version():
    request = {"version": 1, "request_id": 77, "method": {"method": "ping"}}
    result = subprocess.run([str(BIN), "session", "--stdio"], input=json.dumps(request) + "\n", capture_output=True, text=True)
    assert result.returncode == 0
    response = json.loads(result.stdout)
    assert response["request_id"] == 77
    assert response["result"]["Err"]["code"] == "unsupported_version"
