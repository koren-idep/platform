# Koren Tool Manager (Phase 9.8)

`koren-tools` provides language-neutral external tool declarations, bounded local discovery,
strict precedence resolution, and verified managed installations.

## Security & Trust Model

- **Declaration != Installation != Execution**: Declaring `[[tools]]` in `koren-extension.toml`
  or resolving a tool via `koren.tools.resolve(id)` never downloads or executes a binary.
- **Host Trust Policy**: Extension-supplied download URLs are untrusted by default. Managed
  installations require HTTPS (or explicit local test URLs in tests), an exact pinned version,
  a host-approved URL prefix, and a valid 64-character SHA-256 digest.
- **Safe Staging & Extraction**: Downloads stage outside workspace folders, reject path traversal
  (`..`), absolute paths, and symlinks in `.tar.gz` archives, verify SHA-256 before atomic
  placement, and verify executable integrity on load.

## Extension Manifest & Lua SDK

Extensions declare `capabilities = ["tools"]` and `[[tools]]`:

```toml
[[tools]]
id = "lune"
name = "Lune"
description = "Standalone Luau runtime"
category = "runtime"
optional = true
executables = ["lune"]
```

In Lua (`Capability::Tools`):

```lua
local tool = koren.tools.resolve("lune")
if tool then
    -- tool.id, tool.executable, tool.available
end
```

Run configurations (`tool = "lune"`), LSP servers (`command = "tool:lua-language-server"`), and
DAP adapters (`command = "tool:lldb-dap"`) resolve through `ToolManager` before passing the exact
resolved executable to the host's execution approval flow.

## Resolution Precedence

1. Configuration-specific override
2. Workspace selection
3. User default selection
4. Compatible detected executable on `PATH` or `discovery_hints`

Pinned managed versions never silently fall back if missing or modified on disk.
