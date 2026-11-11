# Koren Code Editor

Koren Code Editor is the universal desktop code editor built on Koren IDE Platform. It is language-neutral; the included Rust syntax integration and Cargo provider are the first language-oriented contributions. Koren IDE, the future Rust-specialized distribution, is outside this application's Phase 5 scope.

## Development

Prerequisites are Rust 1.85 or later, Node.js 22.12 or later, and (on Linux) GTK 3 and WebKitGTK 4.1 development packages. On Fedora install `gtk3-devel` and `webkit2gtk4.1-devel`.

```sh
npm ci --prefix apps/koren-code/frontend
cargo run --locked -p koren-code
```

Cargo invokes the frontend production build through `build.rs`. In debug builds, the host serves built assets from `frontend/dist` (or from `KOREN_CODE_ASSET_DIR` when set), so changes are visible after rebuilding. Release builds embed the built assets in the executable and do not need a Vite server or network connection.

## Checks

```sh
npm run --prefix apps/koren-code/frontend typecheck
npm run --prefix apps/koren-code/frontend lint
npm test --prefix apps/koren-code/frontend
npm run --prefix apps/koren-code/frontend build
cargo test --locked -p koren-code
cargo build --release --locked -p koren-code
```

The full repository verification is `python3 scripts/pr_check.py`. On Linux, the desktop launch smoke check is `xvfb-run -a python3 scripts/desktop_smoke.py target/debug/koren-code`.

## Manual Linux acceptance

Run `cargo run --locked -p koren-code`, open the repository root from **File**, and verify the explorer loads the root without recursively enumerating it. Expand directories, open a Rust source file, open a second file, and switch tabs. Make rapid edits in both tabs, save with Ctrl+S, then verify the file contents and dirty markers. To check conflict handling, modify an open file externally, save in the editor, inspect the disk preview, and choose reload or overwrite explicitly. Switch dark/light themes and resize/collapse the explorer and bottom panel.

For task authorization, select **Build → Discover Cargo**, inspect the Cargo project and tasks, request a task, verify the exact command and environment variable names in the approval dialog, and exercise both Deny and Approve once. Approved runs should show bounded output, status, exit result, and cancellation control. Closing a dirty document or the app should offer explicit save/discard/cancel choices.

The automated desktop smoke test verifies window creation and collects startup/RSS diagnostics; it does not automate those interactive steps. Windows and macOS desktop behavior remains unverified.
