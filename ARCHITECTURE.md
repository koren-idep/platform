# Architecture

## Crate responsibilities and dependency directions

The dependency graph points from consumers to the lower-level crates they use:

```text
koren-headless ──> koren-core
       │           koren-protocol
       ├─────────> koren-commands ──> koren-core / koren-protocol (permitted)
       └─────────> koren-workspace ─> koren-core / koren-protocol (permitted)

koren-core       (no internal crate dependencies)
koren-protocol   (no internal crate dependencies)
```

`koren-core` owns lifecycle and minimal context primitives. `koren-protocol` owns typed, versioned internal request and response contracts. `koren-commands` provides command registration, enumeration, and invocation. `koren-workspace` handles explicit local directory access. `koren-headless` wires those public crates together as a small executable.

The Python architecture test reads Cargo metadata and enforces these directions. Lower-level crates cannot depend on the headless application or higher-level implementations. Circular dependencies are rejected by the same allowlist.

## Compiled products and runtime extensions

Koren IDE and other products are planned as compiled Rust applications using platform crates and providing product-specific project models, language integrations, and build support. Runtime extensions will be loaded by the host without rebuilding it. The initial planned extension language is Lua 5.4 via `mlua`; no extension loader or scripting runtime exists yet. WebAssembly support is a possible later direction.

## Frontend boundary

All crates in this foundation are frontend independent. Future desktop code is expected to live outside the core platform boundary and use Tao for windows, Wry/WebKitGTK on Linux, and HTML/CSS/TypeScript for presentation. UI, WebView, and JavaScript dependencies are forbidden in platform core crates. The headless executable demonstrates that backend functionality is callable without a frontend.

## Protocol evolution

The internal protocol starts at version 1 and uses strongly typed Rust request methods, response values, and structured error codes. Its schema is experimental, not a permanent compatibility promise. A breaking contract change should update the protocol version and compatibility tests together. Unknown versions are rejected explicitly. Serialization currently uses Serde; JSON is used at the CLI boundary for integration and machine-readable output.

## Future interfaces

Language services and build/debug integrations will be introduced behind narrowly scoped platform interfaces in later phases. LSP, DAP, agent interfaces (including ACP or MCP), and any associated process or network orchestration are planned only; none are part of the current workspace. Future interfaces must preserve the dependency directions and frontend separation above.
