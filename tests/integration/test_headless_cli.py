import json
import pathlib
import subprocess
import sys
import time

ROOT = pathlib.Path(__file__).resolve().parents[2]
BIN = ROOT / "target" / "debug" / ("koren-headless.exe" if sys.platform == "win32" else "koren-headless")


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


def test_session_project_registry_and_task_authorization(tmp_path):
    (tmp_path / "src").mkdir()
    process = subprocess.Popen([str(BIN), "session", "--stdio"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    def request(i, method):
        process.stdin.write(json.dumps({"version": 2, "request_id": i, "method": method}) + "\n")
        process.stdin.flush()
        return json.loads(process.stdout.readline())
    try:
        request(1, {"method": "open_workspace", "params": {"path": str(tmp_path)}})
        project = request(2, {"method": "register_project", "params": {"display_name": "Demo", "root": "src", "provider_id": "fixture", "configuration_path": None}})
        assert project["result"]["Ok"]["value"]["display_name"] == "Demo"
        assert len(request(3, {"method": "list_projects"})["result"]["Ok"]["value"]) == 1
        registered = request(4, {"method": "register_task", "params": {
            "key": "print", "display_name": "Print", "category": "custom",
            "executable": sys.executable, "arguments": ["-c", "print('task-output')"],
            "working_directory": ".", "environment": {"KOREN_PRIVATE_TEST_VALUE": "do-not-serialize"},
            "timeout_ms": 5000, "project_id": 1,
        }})
        task_id = registered["result"]["Ok"]["value"]["task_id"]
        assert "do-not-serialize" not in json.dumps(request(5, {"method": "list_tasks"}))
        denied = request(6, {"method": "start_task", "params": {"task_id": task_id}})
        assert denied["result"]["Err"]["code"] == "unauthorized"
        removed = request(7, {"method": "unregister_project", "params": {"project_id": 1}})
        assert removed["result"]["Ok"]["value"]["project_id"] == 1
    finally:
        process.stdin.close()
        process.wait(timeout=5)
        assert process.stderr.read() == ""


def test_session_runs_approved_task_and_collects_output(tmp_path):
    process = subprocess.Popen([str(BIN), "session", "--stdio", "--allow-task-execution"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    def request(i, method):
        process.stdin.write(json.dumps({"version": 2, "request_id": i, "method": method}) + "\n")
        process.stdin.flush()
        return json.loads(process.stdout.readline())
    try:
        request(1, {"method": "open_workspace", "params": {"path": str(tmp_path)}})
        task_id = request(2, {"method": "register_task", "params": {
            "key": "print", "display_name": "Print", "category": "run",
            "executable": sys.executable, "arguments": ["-c", "import sys; print('task-out'); print('task-err', file=sys.stderr)"],
            "working_directory": ".", "environment": {}, "timeout_ms": 5000, "project_id": None,
        }})["result"]["Ok"]["value"]["task_id"]
        run_id = request(3, {"method": "start_task", "params": {"task_id": task_id}})["result"]["Ok"]["value"]["run_id"]
        deadline = time.monotonic() + 5
        response = None
        while time.monotonic() < deadline:
            response = request(4, {"method": "task_output", "params": {"run_id": run_id}})
            snapshot = response["result"]["Ok"]["value"]
            if snapshot["state"] == "succeeded":
                break
        assert "task-out" in snapshot["stdout"]
        assert "task-err" in snapshot["stderr"]
        assert snapshot["state"] == "succeeded"
    finally:
        process.stdin.close()
        process.wait(timeout=5)
        assert process.stderr.read() == ""


def test_session_explicit_bounded_marker_discovery(tmp_path):
    project = tmp_path / "nested" / "demo"
    project.mkdir(parents=True)
    (project / "project.marker").write_text("this content is never executed", encoding="utf-8")
    requests = [
        {"version": 2, "request_id": 1, "method": {"method": "open_workspace", "params": {"path": str(tmp_path)}}},
        {"version": 2, "request_id": 2, "method": {"method": "discover_projects", "params": {
            "provider_id": "fixture.marker", "marker_filename": "project.marker",
            "max_directories": 16, "max_projects": 4, "max_depth": 8,
        }}},
    ]
    payload = "".join(json.dumps(item) + "\n" for item in requests)
    result = subprocess.run([str(BIN), "session", "--stdio"], input=payload, capture_output=True, text=True, timeout=5)
    assert result.returncode == 0, result.stderr
    responses = [json.loads(line) for line in result.stdout.splitlines()]
    discovered = responses[1]["result"]["Ok"]["value"]
    assert len(discovered) == 1
    assert discovered[0]["root"] == str(pathlib.Path("nested") / "demo")
    assert discovered[0]["configuration_path"] == str(pathlib.Path("nested") / "demo" / "project.marker")


def test_cargo_workspace_discovery_and_approved_offline_metadata(tmp_path):
    (tmp_path / "Cargo.toml").write_text(
        '[workspace]\nmembers = ["crates/*"]\nresolver = "2"\n', encoding="utf-8"
    )
    package = tmp_path / "crates" / "demo"
    (package / "src").mkdir(parents=True)
    (package / "Cargo.toml").write_text(
        '[package]\nname = "demo"\nversion = "0.1.0"\nedition = "2021"\n', encoding="utf-8"
    )
    (package / "src" / "main.rs").write_text("fn main() {}\n", encoding="utf-8")

    process = subprocess.Popen(
        [str(BIN), "session", "--stdio"],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
    )
    request_id = 0

    def request(method):
        nonlocal request_id
        request_id += 1
        process.stdin.write(json.dumps({"version": 2, "request_id": request_id, "method": method}) + "\n")
        process.stdin.flush()
        return json.loads(process.stdout.readline())

    try:
        assert "Ok" in request({"method": "open_workspace", "params": {"path": str(tmp_path)}})["result"]
        discovered = request({"method": "discover_cargo_projects", "params": {
            "max_directories": 32, "max_projects": 8, "max_depth": 8,
        }})["result"]["Ok"]["value"]
        assert len(discovered) == 1
        cargo_project = discovered[0]
        assert cargo_project["is_workspace"] is True
        assert [member["name"] for member in cargo_project["members"]] == ["demo"]
        project_id = cargo_project["project"]["project_id"]
        info = request({"method": "cargo_project_info", "params": {"project_id": project_id}})
        assert info["result"]["Ok"]["value"]["members"][0]["version"] == "0.1.0"

        task_list = request({"method": "list_tasks"})["result"]["Ok"]["value"]
        metadata_task = next(task for task in task_list if task["key"].endswith("metadata"))
        assert {task["category"] for task in task_list} >= {"build", "check", "test", "clean"}
        requested = request({"method": "request_task_execution", "params": {"task_id": metadata_task["task_id"]}})
        approval = requested["result"]["Ok"]["value"]
        assert requested["result"]["Ok"]["kind"] == "approval_required"
        assert "--offline" in approval["arguments"]
        pending = request({"method": "list_pending_approvals"})["result"]["Ok"]["value"]
        assert [entry["request_id"] for entry in pending] == [approval["request_id"]]

        approved = request({"method": "approve_task_execution", "params": {"request_id": approval["request_id"]}})
        run_id = approved["result"]["Ok"]["value"]["run_id"]
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            result = request({"method": "task_output", "params": {"run_id": run_id}})
            snapshot = result["result"]["Ok"]["value"]
            if snapshot["state"] != "running":
                break
        assert snapshot["state"] == "succeeded", snapshot
        assert any(package["name"] == "demo" for package in json.loads(snapshot["stdout"])["packages"])
        assert "SECRET" not in json.dumps(approval)

        build_task = next(task for task in task_list if task["category"] == "build")
        pending = request({"method": "request_task_execution", "params": {"task_id": build_task["task_id"]}})
        pending_id = pending["result"]["Ok"]["value"]["request_id"]
        request({"method": "unregister_project", "params": {"project_id": project_id}})
        stale = request({"method": "approve_task_execution", "params": {"request_id": pending_id}})
        assert stale["result"]["Err"]["code"] == "not_found"
        assert "Ok" in request({"method": "close_workspace"})["result"]
    finally:
        process.stdin.close()
        assert process.wait(timeout=10) == 0, process.stderr.read()
        assert process.stderr.read() == ""


def test_task_approval_denial_does_not_spawn_and_hides_environment_values(tmp_path):
    marker = tmp_path / "should-not-exist.txt"
    process = subprocess.Popen(
        [str(BIN), "session", "--stdio"],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
    )
    request_id = 0

    def request(method):
        nonlocal request_id
        request_id += 1
        process.stdin.write(json.dumps({"version": 2, "request_id": request_id, "method": method}) + "\n")
        process.stdin.flush()
        return json.loads(process.stdout.readline())

    try:
        request({"method": "open_workspace", "params": {"path": str(tmp_path)}})
        task_id = request({"method": "register_task", "params": {
            "key": "write-marker", "display_name": "Write marker", "category": "custom",
            "executable": sys.executable,
            "arguments": ["-c", "from pathlib import Path; Path(" + repr(str(marker)) + ").write_text('ran')"],
            "working_directory": ".", "environment": {"API_SECRET": "hidden-value"},
            "timeout_ms": 5000, "project_id": None,
        }})["result"]["Ok"]["value"]["task_id"]
        approval = request({"method": "request_task_execution", "params": {"task_id": task_id}})["result"]["Ok"]["value"]
        assert approval["environment_keys"] == ["API_SECRET"]
        assert "hidden-value" not in json.dumps(approval)
        denied = request({"method": "deny_task_execution", "params": {"request_id": approval["request_id"]}})
        assert denied["result"]["Ok"]["kind"] == "task_denied"
        repeated = request({"method": "approve_task_execution", "params": {"request_id": approval["request_id"]}})
        assert repeated["result"]["Err"]["code"] == "not_found"
        assert not marker.exists()
        assert "Ok" in request({"method": "close_workspace"})["result"]
    finally:
        process.stdin.close()
        assert process.wait(timeout=5) == 0, process.stderr.read()
        assert process.stderr.read() == ""
