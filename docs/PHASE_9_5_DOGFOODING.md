# Phase 9.5 dogfooding record

## Scope and verification

The editor was opened on an isolated disposable worktree of the Koren source repository. Interactive checks used the Linux WebKitGTK desktop window; automated checks are listed separately below. The source worktree was not used for demonstration edits.

Manual WebKitGTK checks completed:

- Opened the Koren repository and browsed its Rust workspace and source tree.
- Opened a new document, pasted Unicode (`😀` and an em dash) through the system clipboard, saved it, renamed it while its tab was open, edited it again, and verified the final bytes on disk.
- Ran project search for `gitRepositoryChoices` in the Koren tree; the panel returned 5 matches across 148 files and 9 skipped binary files.
- Opened Source Control on the rebuilt app, confirmed the root repository appears as `./HEAD`, and saw a clean status in the disposable worktree.
- Discovered the Cargo workspace and ran `Cargo workspace check` after approving its exact command in the graphical prompt. The first attempt stopped at the documented `tsc: command not found` prerequisite because the fresh worktree had no frontend dependencies. After `npm ci` in that worktree, the approved Cargo task succeeded with exit code 0 in 17.41 s.
- Opened and approved an integrated shell through the graphical authorization prompt, entered a command, observed streamed output and a successful shell exit. The shell used an isolated temporary home directory.
- Loaded the bundled Hello and Text Inspection examples, enabled and activated them, and invoked the Hello command through the command palette.
- Inspected the Git panel and found that a repository at the workspace root could not be selected: its empty relative path collided with the placeholder option. The selector now gives the root repository the explicit relative path `.` and displays its branch. A frontend regression test covers root and nested repositories together.

The real-window session did not cover save conflicts, external edits, diagnostic quick fixes, panel resizing, or preference persistence across a desktop restart. Those remain unverified interactively. The root path conversion and rendering are covered by the frontend unit test and the selector was also checked in the rebuilt window.

Automated verification completed after the fix:

- `python3 scripts/check.py` passed (format, workspace check, architecture, frontend type-check/lint/tests/build).
- `cargo +1.85.0 check --workspace --locked` passed.
- `python3 scripts/pr_check.py` passed (strict Clippy, workspace tests and docs, Python tests, frontend checks, clean workspace build, release build, desktop smoke).
- The focused `developerTools.test.ts` suite passed (4 tests); the full frontend suite passed (50 tests).

## Environment limits

`rust-analyzer` is not installed as a component of the active Rust toolchain, so no real rust-analyzer session, live diagnostics, or code-action workflow was verified. Rust syntax highlighting and Cargo integration were available. The host has GDB 17.2, but this session did not complete a GUI-authorized real DAP launch; real debugger compatibility remains unverified. Existing fake-adapter tests continue to provide protocol coverage.

Search, workspace navigation, safe document services, Git operations, terminal authorization, and DAP protocol behavior also have automated service and UI tests. Their presence is not evidence that every full GUI workflow listed above was exercised in this dogfooding session.

## Startup and memory sample

Three consecutive Linux debug-build desktop smoke runs used `scripts/run_desktop_smoke.py` on 2026-11-12. Each run waited for the visible window and sampled the application process tree one second later:

| Run | Time to visible window | Process-tree RSS at +1 s |
|---:|---:|---:|
| 1 | 174.2 ms | 395,056 KiB |
| 2 | 170.4 ms | 395,460 KiB |
| 3 | 174.9 ms | 394,660 KiB |
| Median | 174.2 ms | 395,056 KiB |

This is a repeatable smoke procedure on the local Fedora Linux x86_64 host with Xvfb/Openbox, not a representative production-hardware claim. It reports summed resident set size for the editor process and descendants only. It does not report per-process memory, virtual memory, proportional set size, or shared-page attribution; therefore the sum may double-count shared pages and cannot be described as private memory. The sample did not include an active language server, terminal, or debugger.

The built frontend continues to split the terminal and debugger into on-demand chunks. Their current built sizes are about 332.2 KiB and 12.4 KiB respectively; the editor entry is about 489.8 KiB (501.58 kB decimal), just over Vite's 500 kB warning threshold. The editor entry is central to the initial workflow, and this small threshold overage did not justify adding more loading complexity during stabilization. No bundle optimization was made.

Interactive responsiveness was observed qualitatively: tree navigation, opening files, clipboard paste, save, rename, terminal input/output, and command palette activation completed in the real window. No frame-time or latency instrumentation was collected for those actions. Large-workspace search latency, memory with language services, and terminal/debug adapter memory remain unmeasured.

## Remaining stabilization work

- Re-run the complete check suite after the final documentation and code changes.
- Verify root-level Git selection, extension deactivation cleanup, diagnostics and quick fixes, document conflicts, and reopen persistence in a rebuilt interactive window.
- Add per-process RSS/PSS and virtual-memory reporting to the desktop smoke tooling before making component-level memory claims.
- Complete a real GDB DAP workflow only after it can be launched from the editor's graphical approval flow.
- Add repeatable UI latency fixtures for workspace open, file open, tab switch, search, and terminal/debugger activation.
