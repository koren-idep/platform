# Koren IDE Platform

Koren IDE Platform is a Rust framework for building reusable development environments. The **platform** provides shared backend building blocks; **Koren IDE** is the flagship product that will consume those crates and add Rust focused project and language tooling.

## Status

Phase 0 and a narrow Phase 1 foundation are implemented. The workspace currently contains lifecycle, internal protocol, command registry, local workspace access, and a headless demonstration executable. This is an early experimental foundation; it is not yet a desktop IDE.

## Build and verify

Install stable Rust (minimum 1.85), Python 3, and the development requirements:

```bash
python3 -m pip install -r requirements-dev.txt
python3 scripts/check.py
python3 scripts/pr_check.py
```

Build and run the headless example:

```bash
cargo build --locked -p koren-headless
cargo run --locked -p koren-headless -- --version
cargo run --locked -p koren-headless -- commands list
cargo run --locked -p koren-headless -- command execute core.ping
cargo run --locked -p koren-headless -- workspace info . --json
```

Commands and workspace information support `--json` for machine-readable integration. The executable is a consumer of the same crates intended for compiled IDE applications.

## Planned extension model

Products will be compiled Rust applications that depend on Koren Platform crates. Runtime extensions are planned separately, initially using Lua 5.4 through `mlua`, so extensions can be loaded without rebuilding the host. Neither runtime extensions nor Lua are implemented in this phase. WebAssembly may be considered later.
