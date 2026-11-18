"""Dependency boundary validation for the Koren Cargo workspace."""
import argparse
import json
import subprocess
import sys

ALLOWED_INTERNAL_DEPS = {
    "koren-core": set(),
    "koren-protocol": {"koren-run"},
    "koren-commands": {"koren-core", "koren-protocol"},
    "koren-extensions": set(),
    "koren-lsp": {"koren-documents", "koren-protocol"},
    "koren-inspections": {"koren-protocol"},
    "koren-lua": {"koren-extensions", "koren-inspections", "koren-protocol"},
    "koren-workspace": {"koren-core"},
    "koren-documents": {"koren-core", "koren-protocol", "koren-workspace"},
    "koren-projects": {"koren-core", "koren-workspace", "koren-tasks"},
    "koren-tasks": {"koren-core"},
    "koren-run": set(),
    "koren-cargo": {"koren-projects", "koren-tasks", "koren-workspace"},
    "koren-headless": {"koren-core", "koren-protocol", "koren-commands", "koren-workspace", "koren-documents", "koren-projects", "koren-tasks", "koren-cargo"},
    "koren-code": {"koren-core", "koren-protocol", "koren-commands", "koren-workspace", "koren-documents", "koren-projects", "koren-tasks", "koren-cargo", "koren-extensions", "koren-inspections", "koren-lua", "koren-lsp", "koren-run"},
}
BACKEND_PACKAGES = set(ALLOWED_INTERNAL_DEPS) - {"koren-headless", "koren-code"}
FORBIDDEN_BACKEND_DEPS = {
    "tao", "wry", "tauri", "web-view", "webview", "gtk4", "egui", "iced",
    "web-sys", "wasm-bindgen", "js-sys", "deno_core", "boa_engine",
    "javascriptcore-rs", "wasmtime", "wasmer", "lsp-server",
    "tower-lsp", "lsp-types", "lsp_types", "dap", "acp", "mcp",
    "rust-analyzer", "ra_ap_syntax", "tree-sitter", "tree-sitter-rust",
    "syntect", "cargo_metadata", "rls",
}


def violations(metadata):
    """Return internal dependency edges that violate the documented graph."""
    names = set(ALLOWED_INTERNAL_DEPS)
    result = []
    for package in metadata.get("packages", []):
        source = package["name"]
        if source not in names:
            continue
        allowed = ALLOWED_INTERNAL_DEPS[source]
        for dependency in package.get("dependencies", []):
            target = dependency["name"]
            if target in names and target not in allowed:
                result.append(f"{source} -> {target}")
            if source in BACKEND_PACKAGES and target in FORBIDDEN_BACKEND_DEPS:
                result.append(f"{source} -> forbidden dependency {target}")
            if source in BACKEND_PACKAGES and target == "mlua" and source != "koren-lua":
                result.append(f"{source} -> mlua is only allowed in koren-lua")
    return sorted(result)


def cargo_metadata():
    completed = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--no-deps"],
        check=True, capture_output=True, text=True,
    )
    return json.loads(completed.stdout)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--metadata-file", help="validate a saved Cargo metadata document")
    args = parser.parse_args()
    try:
        if args.metadata_file:
            with open(args.metadata_file, encoding="utf-8") as source:
                metadata = json.load(source)
        else:
            metadata = cargo_metadata()
        errors = violations(metadata)
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"architecture check failed: {error}", file=sys.stderr)
        return 1
    if errors:
        print("forbidden crate dependency direction(s): " + ", ".join(errors), file=sys.stderr)
        return 1
    print("architecture boundaries passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
