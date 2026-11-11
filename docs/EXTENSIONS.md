# Extension authoring

Koren Code Editor and future Koren IDE distributions share the same extension APIs. Extensions are local directories containing `koren-extension.toml` and an entry point. Copy an extension directory into the configured extension root (by default `$XDG_DATA_HOME/koren/extensions`, `~/.local/share/koren/extensions`, `%APPDATA%/Koren/extensions`, or `~/Library/Application Support/Koren/extensions`). `KOREN_EXTENSION_DIR` can select a different root. The editor scans only immediate children of this directory when the Extensions manager is opened; it never discovers code in a workspace.

## Manifest

Manifests are TOML and currently use schema version `1`. `api_version` is independent from the application version and must have the same major version as the host extension API, with a version no newer than the host. Unknown manifest fields and invalid identifiers, paths, capabilities, and language entries are rejected.

```toml
schema_version = 1
id = "org.example.hello"
name = "Hello from Koren"
publisher = "Example publisher"
version = "0.1.0"
description = "A sample extension."
entry_point = "main.lua"
api_version = "1.0.0"
capabilities = ["commands", "workspace_read", "ui_contributions"]

[activation]
on_startup = false
on_workspace_open = false

[[languages]]
id = "example"
name = "Example Language"
extensions = ["ex"]

[[language_servers]]
id = "example-server"
command = "example-language-server"
arguments = ["--stdio"]
languages = ["example"]
file_extensions = ["ex"]
```

The language and server tables are optional. A server declaration is data only. It does not start the executable, and its `command` and `arguments` are never parsed by a shell. Environment overrides may also be declared in `[[language_servers]]`; the host approval view shows their names, never values.

## Lua entry point and command

The entry point returns a module table with optional `activate()` and `deactivate()` functions. Commands and menus are staged during activation and become visible together after activation succeeds.

```lua
local extension = {}

function extension.activate()
  koren.commands.register("hello", "Hello", "Report the workspace", function(args, context)
    return "Workspace: " .. (context.workspace_root or "(none)")
  end)
  koren.ui.add_menu_action("hello", "Hello", "tools")
end

function extension.deactivate()
  -- Host-owned registrations are cleared after this callback.
end

return extension
```

`context.workspace_root` and `context.active_document` are snapshots supplied for a command call. Declaring `workspace_read` exposes `koren.workspace.root()`. `document_read` exposes `koren.document.active()` and `koren.document.read(id)`. `document_edit` exposes `koren.document.request_edit(id, revision, start_line, start_utf16_column, end_line, end_utf16_column, replacement)`. The host applies requests through the revision-aware document manager; stale edits fail its normal revision check. Declaring `events` permits `koren.events.on(name, callback)` for the documented workspace and document lifecycle events.

## Lifecycle, permissions, and trust

Discovery reads metadata but does not execute code. Extensions start disabled. Enable changes their state to `installed`; activation is a separate explicit action. States are `disabled`, `installed`, `activating`, `active`, and `failed`. Activation contributions are transactional. A failed activation reports an error and retains no command, menu, or event registrations. Disabling calls `deactivate()` on a best-effort basis and always removes host-owned contributions.

Manifest activation conditions are parsed and preserved as metadata, but this initial desktop host requires an explicit Activate action; it does not yet schedule automatic condition-driven activation.

Lua exposes table, string, UTF-8, math, and coroutine libraries. It does not expose `io`, `os`, `debug`, `package`, `require`, file loading, process, or network APIs. Each VM has a 32 MiB memory limit and a 5,000,000-instruction budget per activation or callback. This is not an operating-system security sandbox: extensions execute in-process, so only install extensions you trust. Lua cannot start tasks or processes and cannot bypass task approvals through the host APIs.

Starting any declared language server requires explicit host approval, including configurations from first-party extensions. The approval is for the exact executable, arguments, working directory, and environment configuration, and is remembered only for that exact configuration during the active workspace session. Changing the configuration requires new approval; closing the workspace stops servers and clears those approvals. Approval is not process isolation; servers run with the editor user's OS permissions.

Document open, revision changes, and close are synchronized with active compatible servers using the document manager's snapshots and UTF-16 positions. Diagnostics are replaced per provider and stale versions are discarded. Hover, completion, and definition requests use advertised capabilities and have a 150 ms desktop request budget. Server initialization uses a five-second timeout on the platform worker, so a slow or hung server can delay other bridge operations while it initializes; an asynchronous server host remains a follow-up.

The public Rust crates are currently experimental. Protocol and extension API compatibility checks are implemented, but this release does not promise a stable compiled-plugin ABI. Extensions should use the manifest and typed host APIs; arbitrary HTML/JavaScript injection, dynamic Rust libraries, and WebAssembly are not supported.

## Local development

Copy `examples/extensions/org.example.hello` into the extension root, restart or reopen the Extensions manager, enable it, then activate it. Its command appears in the command palette and Tools menu. The optional `org.koren.rust-analyzer` example uses the same configuration path for any LSP-compatible editor host; install `rust-analyzer` separately before starting it. It is not bundled and is not required for editing.
