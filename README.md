# Koren IDE Platform

Koren IDE Platform is a Rust framework for building reusable development environments. The **platform** provides shared backend building blocks; **Koren IDE** is the flagship product planned to consume those crates and add Rust focused project and language tooling.

## Status

Phases 0 through 3 provide lifecycle primitives, an experimental internal protocol, command registration, demand-driven workspace navigation, revisioned text documents, language-neutral projects and discovery providers, bounded direct-process task execution, and a headless session. This is a backend foundation, not yet a desktop IDE. No language-specific project/build integration or runtime extension loader is implemented.

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

## Headless workspace, projects, documents, and tasks

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

The example edits a temporary file and removes it on exit. Project operations include registration, listing, lookup, removal, and explicit bounded marker discovery; task operations include definition registration/listing, start, status/output, cancellation, and cleanup. They use the same version-2 request envelope. Adding these tagged operations is additive: existing version-2 request and response encodings did not change, so the protocol remains version 2.

Tasks use a direct executable and argument vector; there is no shell expansion. Registering a task does not authorize it to run. The headless host denies starts unless launched with session --stdio --allow-task-execution; that local startup option grants the session client permission to start tasks as an interactive user. Other Rust hosts implement TaskAuthorizer to distinguish interactive users, extensions, and future agents. Task summaries omit environment values.

The task service retains up to 1 MiB per output stream and four concurrent processes by default. Completed output remains in memory until cleanup_task or service shutdown. Closing the workspace or dropping the service requests cancellation of direct child processes. Cancellation does not portably terminate process descendants; see ARCHITECTURE.md. The workspace path policy is not a security sandbox.

## Workspace and document behavior

Workspace paths are relative to one open root. Parent traversal and absolute child paths are rejected. Symbolic links appear in listings but cannot be traversed. Directory listings are non-recursive and limited to 10,000 entries by default; Rust consumers can configure the limit.

Documents use Ropey buffers and UTF-16 line/column edit coordinates. A manager returns the existing document id when a path is opened again. Revisions advance once per successful edit batch, and stale edits are rejected. UTF-8 BOM and LF/CRLF bytes are retained. Invalid UTF-8, NUL-containing files, and files over 16 MiB by default are rejected.

Save checks a backing-file fingerprint before writing. A conflict leaves both disk and buffer untouched until the caller explicitly chooses reload or overwrite. Saving writes a same-directory temporary file and replaces the target; the file is synchronized before replacement, but the containing directory is not synchronized, so crash durability is not guaranteed. Filesystem races from hostile concurrent modifications are outside the path policy's guarantees.

Run reproducible, non-gating backend benchmarks with:

```bash
cargo bench --locked -p koren-documents --bench phase2
cargo bench --locked -p koren-projects --bench phase3
```

The checked-in backend benchmark baselines in BENCHMARKS.md record fixture sizes and measurement environments.

## Project and task lifecycle

Workspaces own opened filesystem roots. Logical projects are explicit records associated with a workspace and can be nested or coexist under different providers. Project IDs are local to a ProjectManager and do not persist across sessions. Providers are registered explicitly; discovery is invoked by the host with directory, depth, and result limits. The marker-file provider is only a generic fixture and never reads or executes marker content.

Build providers can expose zero or more generic TaskDefinition capabilities. The task service also accepts manually configured definitions with no project. See ARCHITECTURE.md for state, events, authorization, and platform limits.

## Planned extension model

IDE products will be compiled Rust applications that depend on platform crates. Runtime extensions are planned separately, initially using Lua 5.4 through `mlua`; no extension runtime is implemented. WebAssembly may be considered later.
