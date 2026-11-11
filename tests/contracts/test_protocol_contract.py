import json
import pathlib
import subprocess

ROOT = pathlib.Path(__file__).resolve().parents[2]
BIN = ROOT / "target" / "debug" / "koren-headless"


def test_ping_json_matches_versioned_response_contract():
    result = subprocess.run([str(BIN), "command", "execute", "core.ping", "--json"], capture_output=True, text=True)
    assert result.returncode == 0, result.stderr
    response = json.loads(result.stdout)
    assert response == {"version": 1, "request_id": 2, "result": {"Ok": {"kind": "command_output", "value": "pong"}}}


def test_unknown_command_is_structured_and_nonzero():
    result = subprocess.run([str(BIN), "command", "execute", "missing.command", "--json"], capture_output=True, text=True)
    assert result.returncode != 0
    response = json.loads(result.stdout)
    assert response["version"] == 1
    assert response["result"]["Err"]["code"] == "not_found"


def test_cli_rejects_invalid_protocol_json_mode_position():
    result = subprocess.run([str(BIN), "--json", "command", "execute", "core.ping"], capture_output=True, text=True)
    assert result.returncode != 0
