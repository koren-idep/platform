# Koren IDE Platform

Koren IDE Platform is a Rust framework for building reusable development environments. The **platform** provides shared backend building blocks; **Koren IDE** is the flagship product planned to consume those crates and add Rust focused project and language tooling.

## Status

Phases 0 through 2 provide lifecycle primitives, an experimental internal protocol, command registration, demand-driven workspace navigation, revisioned text documents, conflict-aware saves, and a headless editing session. This is a backend foundation, not yet a desktop IDE. No language-specific project model or runtime extension loader is implemented.

## Build and verify

Install stable Rust (minimum 1.85), Python 3, and the development requirements:

```bash
python3 -m pip install -r requirements-dev.txt
python3 scripts/check.py
python3 scripts/pr_check.py
```

Run the existing headless commands:

```bash
cargo run --locked -p koren-headless -- --version
cargo run --locked -p koren-headless -- commands list
cargo run --locked -p koren-headless -- command execute core.ping
cargo run --locked -p koren-headless -- workspace info . --json
```

## Headless document editing

`koren-headless session --stdio` reads one versioned JSON request per input line and writes one response per line. The same process retains the workspace and document buffers across requests. For example:

```bash
cargo build --locked -p koren-headless
demo_dir="$(mktemp -d)"
trap 'rm -rf "$demo_dir"' EXIT
printf 'fn main() {}\n' > "$demo_dir/example.rs"
target/debug/koren-headless session --stdio <<JSON
{"version":2,"request_id":1,"method":{"method":"open_workspace","params":{"path":"$demo_dir"}}}
{"version":2,"request_id":2,"method":{"method":"open_document","params":{"path":"example.rs"}}}
{"version":2,"request_id":3,"method":{"method":"read_document","params":{"document_id":1}}}
{"version":2,"request_id":4,"method":{"method":"apply_document_edits","params":{"document_id":1,"expected_revision":0,"edits":[{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":0}},"new_text":"# Local draft\n"}]}}}
{"version":2,"request_id":5,"method":{"method":"save_document","params":{"document_id":1,"expected_revision":1}}}
{"version":2,"request_id":6,"method":{"method":"close_document","params":{"document_id":1,"discard_unsaved_changes":false}}}
{"version":2,"request_id":7,"method":{"method":"close_workspace"}}
JSON
```

The example edits a temporary file and removes it on exit. Session errors are returned as structured protocol responses. Process diagnostics go to standard error.

## Workspace and document behavior

Workspace paths are relative to one open root. Parent traversal and absolute child paths are rejected. Symbolic links appear in listings but cannot be traversed. Directory listings are non-recursive and limited to 10,000 entries by default; Rust consumers can configure the limit.

Documents use Ropey buffers and UTF-16 line/column edit coordinates. A manager returns the existing document id when a path is opened again. Revisions advance once per successful edit batch, and stale edits are rejected. UTF-8 BOM and LF/CRLF bytes are retained. Invalid UTF-8, NUL-containing files, and files over 16 MiB by default are rejected.

Save checks a backing-file fingerprint before writing. A conflict leaves both disk and buffer untouched until the caller explicitly chooses reload or overwrite. Saving writes a same-directory temporary file and replaces the target; the file is synchronized before replacement, but the containing directory is not synchronized, so crash durability is not guaranteed. Filesystem races from hostile concurrent modifications are outside the path policy's guarantees.

Run reproducible, non-gating backend benchmarks with:

```bash
cargo bench --locked -p koren-documents --bench phase2
```

The checked-in [Phase 2 benchmark baseline](BENCHMARKS.md) records the fixture sizes and measurement environment.

## Planned extension model

IDE products will be compiled Rust applications that depend on platform crates. Runtime extensions are planned separately, initially using Lua 5.4 through `mlua`; no extension runtime is implemented. WebAssembly may be considered later.
