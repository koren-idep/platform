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
