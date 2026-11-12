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

`context.workspace_root` and `context.active_document` are snapshots supplied for a command call. Declaring `workspace_read` exposes `koren.workspace.root()`. `document_read` exposes `koren.document.active()` and `koren.document.read(id)`. `document_edit` exposes `koren.document.request_edit(id, revision, start_line, start_utf16_column, end_line, end_utf16_column, replacement)`. The host applies requests through the revision-aware document manager; stale edits fail its normal revision check. Declaring `events` permits `koren.events.on(name, callback)` for workspace and document lifecycle events plus `settings_changed`.

## UI contributions and settings

The shared `ContributionBuilder` / `ContributionRegistry` own commands, menus, panels, settings, themes, diagnostics, and wizard declarations by extension ID. The host publishes them only after activation succeeds and removes the whole owner set on deactivation. Identifiers are qualified with the manifest ID; collisions, invalid schemas, unowned commands, and undeclared capabilities reject activation.

```lua
koren.commands.register_with_options("refresh", "Refresh", "Refresh workspace details", {
  shortcut = "Ctrl+Alt+R",
  enablement = "workspace_open", -- always | workspace_open | active_document
}, function(_args, context)
  local root = context.workspace_root or "(none)"
  koren.ui.update_tool_window("workspace", "Workspace: " .. root)
  return root
end)

koren.ui.register_tool_window("workspace", "Workspace Info", "Run Refresh to load the workspace path.")
koren.ui.add_menu_action("refresh", "Refresh", "tools")
koren.ui.add_menu_action("refresh", "Refresh", "project_explorer_context")
koren.settings.register_boolean("show_path", "Show workspace path", true)
koren.settings.register_integer("sample_limit", "Sample limit", 20, 1, 100)
koren.settings.register_text("label", "Panel label", "Workspace", 80)
koren.settings.register_choice("mode", "Mode", "safe", {"safe", "fast"})
koren.themes.register("midnight", "Midnight", "dark", { accent = "#62a8ff" })
koren.diagnostics.provide_detail("type-help", "rust-analyzer", "E0308", "Type mismatch", "Compare the expected and supplied types.")
```

`koren.settings.get(local_id)` reads the host-validated effective value. Settings changed in the editor UI are persisted under the user's Koren configuration directory and emit the `settings_changed` event to opted-in extensions. A tool window appears in View and opens only on user action; its ID and open state survive catalog refreshes. If an extension is disabled or removed, its panel is pruned from the saved open-panel list. Lua panel updates are bounded plain-text updates to an already registered panel.

The UI model is declarative: text, headings, buttons, lists, tables, text/select/checkbox fields, and status indicators. Buttons route through a registered command in the Rust host. Themes may override only approved design tokens with `#RRGGBB` colors. No contribution can insert HTML, JavaScript, CSS, start a process, or acquire filesystem permission. Lua remains in-process and is not an OS security boundary.

The Rust SDK also exposes a `ProjectWizardContribution` and bounded form components. A host must validate submissions and request filesystem or task work through its authorized services. Phase 7's generic renderer validates supported field values but does not create project files; a contributed form is never treated as proof of successful creation.

## Lifecycle, permissions, and trust

Discovery reads metadata but does not execute code. Extensions start disabled. Enable changes their state to `installed`; activation is a separate explicit action. States are `disabled`, `installed`, `activating`, `active`, and `failed`. Activation contributions are transactional. A failed activation reports an error and retains no command, menu, or event registrations. Disabling calls `deactivate()` on a best-effort basis and always removes host-owned contributions.

Manifest activation conditions are parsed and preserved as metadata, but this initial desktop host requires an explicit Activate action; it does not yet schedule automatic condition-driven activation.

Lua exposes table, string, UTF-8, math, and coroutine libraries. It does not expose `io`, `os`, `debug`, `package`, `require`, file loading, process, or network APIs. Each VM has a 32 MiB memory limit and a 5,000,000-instruction budget per activation or callback. This is not an operating-system security sandbox: extensions execute in-process, so only install extensions you trust. Lua cannot start tasks or processes and cannot bypass task approvals through the host APIs.

Starting any declared language server requires explicit host approval, including configurations from first-party extensions. The approval is for the exact executable, arguments, working directory, and environment configuration, and is remembered only for that exact configuration during the active workspace session. Changing the configuration requires new approval; closing the workspace stops servers and clears those approvals. Approval is not process isolation; servers run with the editor user's OS permissions.

Document open, revision changes, and close are synchronized with active compatible servers using the document manager's snapshots and UTF-16 positions. Diagnostics are replaced per provider and stale versions are discarded. Hover, completion, and definition requests use advertised capabilities and have a 150 ms desktop request budget. Each authorized language server has an actor thread and bounded FIFO mailbox; initialization, transport I/O, feature waits, and shutdown run outside the shared platform worker.

## Phase 8 inspection and language actions

Rust providers can use the `koren-inspections` crate to declare stable rule metadata and produce findings against immutable document snapshots. Findings retain provider and rule identities, UTF-16 ranges, revisions or project snapshot IDs, explanations, related locations, and proposed text actions. Providers are registered and removed explicitly. Document and project jobs are opt-in, bounded, cancellable, and never walk the filesystem or launch tools. Project hosts must supply their own snapshot. Profiles preserve unavailable rule settings and support typed options, global choices, and workspace overrides. Generic suppressions are host state and do not rewrite source files.

Lua extensions with both `diagnostics` and `document_read` capabilities can register rules with `koren.inspections.register_rule(definition, callback)`. Rule IDs are namespaced with the manifest ID. Definitions accept `id`, `title`, `description`, `category`, optional `languages`, optional `severity` (`information`, `hint`, `warning`, or `error`), and typed `options`. The callback receives an immutable `document` (`path`, `language`, `text`, `document_id`, and `revision`) and effective `settings` (`enabled`, `severity`, and `options`). It returns an array of findings with zero-based UTF-16 ranges, a `message`, and optional `category`, `explanation`, `related`, and `replacement`. A replacement becomes a code action containing a text edit pinned to the inspected document revision; the host must preview and confirm it through the normal document edit API. Callbacks run on the bounded inspection worker service, not on the UI request thread. Work is limited by the Lua VM instruction and memory quotas, and the job cancellation token is checked between rules and results.

`examples/extensions/org.example.text-inspection` is a deterministic plain-text example. It reports each `TODO` occurrence and proposes replacing it with `DONE`; it does not perform semantic language analysis. Install it like the Hello example. Its finding and edit proposal are provided by the shared inspection engine. Hosts must still provide the confirmation preview and apply the edit through the revision-aware document pipeline.

To build the same workflow, declare both `diagnostics` and `document_read`, register the rule during extension activation, and return findings from the callback using zero-based UTF-16 positions. Use a stable rule ID within the extension namespace and declare the languages and default severity. Findings may add an explanation and related locations; `replacement` creates a revision-bound single-document quick fix. In Koren Code Editor, the Problems panel shows the rule, provider, explanation, and available action. Selecting the action opens a diff preview; only explicit confirmation sends the edit through the desktop document API. That API rechecks the document revision before applying it. A changed or closed document rejects the stale action. Disabling the extension removes its provider and its live findings; profile values for the rule remain stored so they return if the extension is enabled again. Lua callbacks execute under the runtime's instruction and memory budgets in the shared inspection job worker, with cancellation checks around provider execution.

The SARIF adapter accepts and exports a bounded subset of SARIF 2.1.0. Desktop import selects an existing local file, limits input to 5 MiB and 20,000 findings, rejects unsafe paths, and does not fetch external resources. Unsupported fields are returned as warnings. Imported findings are marked as historical and identify their report and source revision when present; they are never presented as current live diagnostics. Export includes fresh document findings and completed project-snapshot results available to the desktop.

LSP requests for code actions and intentions, references, rename, and formatting are capability-gated and remain on per-server actors. `Ctrl+.` requests code actions, `F2` requests rename, `Shift+F12` requests references, and `Ctrl+Shift+F` requests formatting. Each proposed edit requires an explicit preview confirmation and uses the revision-aware document pipeline. Desktop refactoring currently accepts only a single open document; multi-file edits, resource operations, and provider commands are not executed. Rename proposals affecting multiple files fail before any edit.

The public Rust crates are currently experimental. Protocol and extension API compatibility checks are implemented, but this release does not promise a stable compiled-plugin ABI. Extensions should use the manifest and typed host APIs; arbitrary HTML/JavaScript injection, dynamic Rust libraries, and WebAssembly are not supported.

## Phase 9A host services

The Rust platform exposes workspace mutation in `koren-workspace`, bounded text search in `koren-search`, and local Git operations in `koren-git`. Desktop and headless hosts call these same APIs through additive protocol-v2 requests. Workspace changes are coordinated with the revision-aware document manager; dirty buffers block conflicting rename, move, and delete operations. Search is explicit, cancellable, ignore-aware, and returns UTF-16 source columns.

These services are not automatically exposed to Lua. The current capability model does not grant extensions unrestricted filesystem writes, project-wide scanning, staging, or commits. A future host may add narrow extension operations with explicit capability declarations and user confirmation, while reusing the shared Rust services. Extensions must not execute Git through a shell or bypass task/server authorization.

The Git adapter uses `git2` and libgit2 locally. It does not invoke user hooks; commit hooks and remote operations are not part of this phase. The current Code Editor UI exposes only status, bounded text diffs, selected-path stage/unstage, and explicit commits from the staged index.

## Phase 9B terminal permissions

Interactive terminal sessions are host-only capabilities in Koren Code Editor. Lua extensions receive no terminal create/start/write/resize/close API, and terminal output events are excluded from extension callbacks. An extension must not try to bypass this policy through a contributed panel or command. Shell startup requires direct user approval in the desktop; after approval, commands typed into that shell are not individually approved. See [docs/TERMINAL.md](TERMINAL.md) for session lifecycle, transport, and platform constraints.

## Phase 9C debugger contributions

Extensions may declare `debug_adapters` and `debug_configurations` in `koren-extension.toml` when they include the `debug_adapters` capability. Declarations are validated as owned manifest contributions. Profile IDs must be unique within the owner, refer to a declared adapter, and provide a valid launch target or attach process ID. `${workspaceFolder}` is resolved by the host when a workspace is active. Definitions are exposed only while the extension is active; disabling or deactivating it removes the extension's contributions. The host qualifies IDs with the extension ID to avoid collisions.

Registering a DAP adapter is metadata only and never starts a process. Each start must go through the desktop's graphical approval, which covers the adapter and target configuration. Lua retains its existing restrictions: there is no arbitrary process, debugger, or terminal API. See [docs/DEBUGGING.md](DEBUGGING.md) for the manifest example, lifecycle, and support limits.

## Local development

Copy `examples/extensions/org.example.hello` into the extension root, reopen the Extensions manager, enable it, then activate it. The Developer Tools example demonstrates a command, menu and palette entry, user-opened workspace panel, typed setting, safe theme, diagnostic details, and panel refresh. Copy `examples/extensions/org.example.text-inspection` to demonstrate Lua rule registration and a safe replacement proposal. The optional `org.koren.rust-analyzer` example uses the same configuration path for any LSP-compatible editor host; install `rust-analyzer` separately before starting it. It is not bundled and is not required for editing.
