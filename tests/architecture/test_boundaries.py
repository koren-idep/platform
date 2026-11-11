import json
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
from architecture import violations  # noqa: E402


def package(name, *dependencies):
    return {"name": name, "dependencies": [{"name": dep} for dep in dependencies]}


def test_positive_dependency_fixture():
    metadata = {"packages": [
        package("koren-core"), package("koren-protocol"),
        package("koren-commands", "koren-core"),
        package("koren-workspace", "koren-core", "koren-protocol"),
        package("koren-headless", "koren-core", "koren-protocol", "koren-commands", "koren-workspace"),
    ]}
    assert violations(metadata) == []


def test_negative_dependency_fixture_rejects_reverse_edge():
    metadata = {"packages": [package("koren-protocol", "koren-headless")]}
    assert violations(metadata) == ["koren-protocol -> koren-headless"]


def test_checker_fails_for_forbidden_metadata(tmp_path):
    fixture = tmp_path / "metadata.json"
    fixture.write_text(json.dumps({"packages": [package("koren-core", "koren-workspace")]}), encoding="utf-8")
    result = subprocess.run([sys.executable, str(ROOT / "scripts/architecture.py"), "--metadata-file", str(fixture)], capture_output=True, text=True)
    assert result.returncode != 0
    assert "koren-core -> koren-workspace" in result.stderr


def test_repository_cargo_metadata_satisfies_boundaries():
    result = subprocess.run([sys.executable, str(ROOT / "scripts/architecture.py")], cwd=ROOT, capture_output=True, text=True)
    assert result.returncode == 0, result.stderr
