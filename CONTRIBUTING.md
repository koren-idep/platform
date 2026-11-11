# Contributing

## Prerequisites and setup

Use stable Rust 1.85 or newer, Python 3, and pytest from the pinned development requirements:

```bash
python3 -m pip install -r requirements-dev.txt
cargo build --workspace --locked
```

Rust formatting and Clippy are installed by the repository's `rust-toolchain.toml`.

## Style and local checks

Use `cargo fmt` formatting, keep Rust warnings clear, and prefer standard-library solutions when they suffice. Run `python3 scripts/check.py` during development. Before proposing a change, run `python3 scripts/pr_check.py`, which performs the complete PR validation without rewriting source files.

To compare local workspace or text-edit performance, run `cargo bench --locked -p koren-documents --bench phase2`. Update `BENCHMARKS.md` only with the fixture, environment, and measured results from an actual run; do not turn machine-specific results into general performance claims.

Phase 6 extension and fake-LSP microbenchmarks run with `cargo bench --locked -p koren-lua --bench phase6` and `cargo bench --locked -p koren-lsp --bench phase6`. The fake language server is a local Python stdlib fixture, not a real language server. Report the editor process-tree memory separately from any external language-server process.

For extension API and lifecycle details, use [docs/EXTENSIONS.md](docs/EXTENSIONS.md). Keep the language-server client generic, keep all process launches behind explicit host approval, and avoid making the desktop UI or language services depend on a particular language provider.

## Pull requests

Keep each change focused and small. Include regression coverage for fixes and tests plus documentation for new public APIs. Do not change protocol contracts silently. A pull request must pass the full PR check and describe its behavior, test results, and any limitations. Preserve unrelated work and review the final diff for accidental changes.

## Commits

Use single-line Conventional Commit subjects such as `feat(commands): add command registry`. Keep commits atomic and do not combine unrelated work.
