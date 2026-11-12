# Debugging

Koren Code Editor provides a language-neutral Debug Adapter Protocol (DAP) client in `koren-debug`. It launches local stdio adapters through direct executable and argument vectors; it never builds a shell command. The desktop loads no adapter at startup. A profile is registered as metadata, then each start requires a separate graphical approval covering the adapter executable, arguments, working directory, target or process ID, target arguments, environment override names, and workspace.

## Lifecycle and operations

The public Rust API is `DebugManager`, `DebugSessionHandle`, and the typed adapter, configuration, breakpoint, session, and capability structures in `koren-debug`. A session starts lazily after its host-supplied `DebugAuthorizer` approves both adapter and target. The client initializes the adapter, synchronizes breakpoints, launches or attaches, then accepts asynchronous requests for continue, pause, step in/over/out, threads, stack frames, scopes, variables, breakpoint replacement, and stop/disconnect. Frame pages and variable pages are bounded. Variable references carry a stop generation and are rejected after execution resumes or a new stop invalidates them.

Desktop operations use the existing version-2 request correlation and event channel. They include profile registration/listing, start request/approve/deny, session listing, actions, inspection, breakpoints, and stop. Pending approvals expire and are removed when the workspace changes. An extension cannot launch a process by registering a profile. The host explicitly rejects adapter-originated reverse requests, including `runInTerminal`; the client advertises no terminal support to adapters.

The Debug tool window is lazy and provides profile/session selection, state-sensitive controls, threads, stack frames, scopes, expandable variables, breakpoint state, and bounded output. CodeMirror marks source breakpoints and the selected execution line in its gutter. Clicking a breakpoint row replaces that source's breakpoint set; verified, unverified, disabled, and current execution states are represented separately. Breakpoints are line-based and remain attached to their original line through edits; they are not automatically shifted with text edits. Source navigation is allowed only for paths inside the active workspace and does not discard dirty buffers.

## Extension contributions

An extension may declare `debug_adapters` and `debug_configurations` only with the `debug_adapters` capability. Adapter and profile IDs are unique within their extension and qualified by the host as `<extension-id>.<local-id>`. Profiles must reference a contributed adapter and provide a valid launch program or nonzero attach process ID. `${workspaceFolder}` is resolved by the host when a workspace is open. Definitions are visible only while their owner is active; disabling or deactivating an extension removes its registry contributions. Contributions are metadata and do not authorize execution. Lua keeps its existing restricted libraries and does not receive process-launch or terminal operations.

Example `koren-extension.toml`:

```toml
schema_version = 1
id = "org.example.python-debug"
name = "Python Debug Adapter"
publisher = "Example"
version = "0.1.0"
description = "A suggested local Python debug profile"
entry_point = "main.lua"
api_version = "1.0.0"
capabilities = ["debug_adapters"]

[[debug_adapters]]
id = "debugpy"
name = "debugpy"
command = "python3"
arguments = ["-m", "debugpy.adapter"]
working_directory = "${workspaceFolder}"

[[debug_configurations]]
id = "launch-current"
name = "Launch current file"
adapter_id = "debugpy"
request = "launch"
program = "${workspaceFolder}/main.py"
working_directory = "${workspaceFolder}"
stop_on_entry = false
```

The example is schema documentation only. Python/debugpy is not bundled or tested as a compatible adapter.

## Limits and verification

DAP framing, payloads, command queues, output/event queues, sessions, breakpoint counts, argument counts, and page sizes have explicit bounds. A stalled or crashed adapter fails its session instead of holding the document or terminal request path; a deterministic test-only short deadline verifies unanswered requests fail the isolated session. Session events are best-effort when the bounded event channel is full. No compatible DAP adapter (`lldb-dap`, `lldb-vscode`, or CodeLLDB) was present on the Linux development machine when Phase 9C was checked; installed GDB alone does not establish DAP compatibility. Real adapter/compiled-fixture compatibility is therefore unverified. The deterministic fake adapter covers initialization, stop and inspection requests, launch, breakpoints, disconnect, reverse requests, protocol ordering, concurrency, and cleanup.

Linux desktop startup is covered by the Xvfb smoke test. Native WebKitGTK interactive debugger and terminal acceptance is reported separately from automated Rust/frontend checks. Windows and macOS debugger and desktop behavior have not been verified. See [BENCHMARKS.md](../BENCHMARKS.md) for measurements and boundaries.
