# Koren Code Editor and extension roadmap

This document records agreed future direction. These items are not part of Phase 9.5 and do not authorize beginning later phases.

## Code Editor Phase 9.6 — Run Configurations

The universal editor will provide one top-right Play button backed by named run configurations from extensions, project providers, or the user. User-defined entries will support executable, arguments, working directory, and environment overrides with a simple editor. Workspace configurations will persist. Every run continues through the existing host execution approval flow, which presents the exact resolved command and context before starting it.

## Code Editor Phase 9.7 — Tool Manager

Extensions may declare optional external tools. Users can select an existing installation, locate an executable, choose a supported version, approve a managed download, and remove managed versions. Tool download approval and tool execution approval are separate decisions. Large runtimes are not bundled automatically.

## Language extension sequence

Language integrations remain removable even when they ship preinstalled.

1. **Extension Phase 1 — Lua:** The ordinary extension name is `Lua`. Support Lua, LuaJIT, and Luau through the public extension APIs and appropriate external language servers/runtimes. Recognize `.luarc.json`, `.luarc.jsonc`, and `.luaurc`. Use LuaLS and Luau LSP integrations. Lune is optional and is not bundled.
2. **Extension Phase 2 — JSON.**
3. **Extension Phase 3 — TOML.**

The platform and editor APIs should allow these integrations to be installed, updated, disabled, and removed without disabling the core editor.

## Repository boundaries

After initial extension development, the intended repositories are:

- `koren-idep/platform`
- `koren-idep/editor`
- `koren-idep/lua`
- `koren-idep/json`
- `koren-idep/toml`

Repository separation is deferred until after initial extension development. It is independent from Code Editor Phase 9.7 and does not include a registry.

## Deferred products and protocols

- Phase 10: Koren IDE
- Phase 11: ACP/MCP
- Phase 12: Extension Registry

The Extension Registry is explicitly Phase 12. It is not part of repository restructuring or the Tool Manager.
