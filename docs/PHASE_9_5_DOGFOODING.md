# Phase 9.5 dogfooding and reliability record

## Scope and method

Interactive checks used Koren Code Editor on Linux x86_64 with WebKitGTK under Xvfb/Openbox. Debugging and language-server workflows used disposable programs and an isolated home directory. No demonstration edits were made in the Koren repository. Interactive acceptance is reported separately from automated checks.

Earlier Phase 9.5 checks remain recorded below: project search, Unicode editing and saving, rename with an open tab, root-level Git selection, extension commands, graphical shell approval, and approved Cargo execution were exercised in the desktop window. The root repository selector fix and its frontend regression test are in the earlier Phase 9.5 commit history.

## Real GDB DAP

GDB 17.2 was verified locally with `gdb --interpreter=dap`; its DAP initialize request succeeded and the build reported Python support. A temporary Rust program was compiled with debug symbols. Koren was configured to start `/usr/bin/gdb --interpreter=dap` with the fixture executable and working directory. The desktop displayed the exact adapter and target details and required explicit approval before launch.

The real session initialized, accepted a source breakpoint, launched, stopped at that breakpoint, displayed the source execution line, returned `main::main` in the stack, and showed the local `answer = 40`. Step Over advanced execution; Continue completed the fixture and showed `dap fixture: 80`; a separate Stop ended a session in the `terminated` state. The installed GDB emitted `No source file named ...` and reported a pending breakpoint before the executable was loaded, then resolved and hit it after launch. This is a pre-load diagnostic from GDB, not evidence of a failed breakpoint.

The real window exposed a Koren defect before the successful run: editor tabs carry workspace-relative paths, while the DAP breakpoint API requires an absolute source path. Koren rejected the click with `debug breakpoint source must be absolute`. The frontend now resolves workspace-relative source paths before requesting breakpoints; focused tests cover Unix paths, Windows paths, and rejection of a relative workspace root. Fix committed as `82f935b` (`fix(debug): resolve workspace paths for breakpoints`).

This verifies GDB 17.2's native DAP launch path on this Linux host. Other DAP adapters and Windows/macOS desktop behavior were not tested.

## Real rust-analyzer

The installed `rust-analyzer` was the rustup shim and initially had no component installed for the active stable toolchain. The official stable component was installed locally and reported `rust-analyzer 1.99.1 (58cf8f9 2026-10-13)`. A disposable Cargo project and a temporary Rust extension configuration pointed Koren at the direct analyzer executable, with explicit `HOME`, `CARGO_HOME`, `RUSTUP_HOME`, and `PATH` values because the language-server manager starts processes with a cleared environment. Koren displayed a graphical authorization request before starting it. The running analyzer and its proc-macro helper processes were observed; they exited when Koren was closed.

The session produced a real unresolved-name diagnostic in Problems. After editing the source to remove the error, the old diagnostic disappeared. Hover showed rust-analyzer's inferred `i32` type information, and completion offered the local `answer` value. The source was edited rapidly and the editor remained responsive; a second open document was used during the same session. These checks confirm basic initialization, workspace loading, synchronization, diagnostics, hover, completion, and shutdown.

An earlier Ctrl+. request returned real rust-analyzer actions such as “Insert explicit type” and “Promote local to constant,” but no supported immediate text edit. Inspection found that Koren sent code-action requests with an empty diagnostic context. Koren now sends current-revision diagnostics and asks for quick fixes when those diagnostics are present; older protocol-v2 clients may omit the new optional field. A bridge regression fixture only returns its quick fix when both the diagnostic and `quickfix` filter are forwarded. In a later GUI run, rust-analyzer produced a single unresolved-name diagnostic for the corrected disposable fixture. The live run still did not produce a supported edit preview; no real quick-fix preview was completed. Go-to-definition, explicit server restart, and workspace-close shutdown were not conclusively verified interactively.

The restart exercise also reproduced old syntax diagnostics after closing and reopening the same source while the language server remained active. LSP versions had restarted at `1` on each open, allowing a delayed publication from the old open to match the new one. `LspDocumentSync` now uses a monotonically increasing per-server version across opens, edits, and closes; a regression test verifies that an old version cannot match after reopen. The live GUI run on the repaired build showed the unresolved-name diagnostic but did not deterministically reproduce the delayed-publication race again.

The analyzer configuration was specific to the disposable fixture and did not modify the repository's extension catalog or weaken execution approvals. Analyzer-triggered Cargo/build-script behavior was not separately measured.

## External save conflicts

The editor opened a disposable `conflict.txt`; an external process independently replaced it with Unicode text (`external version – 雪`). Koren's save opened the “File changed outside Koren” dialog. Expanding the preview showed the external text. Choosing Reload put that exact text in the editor and left the document clean at a newer revision.

A second external revision was then written while Koren held a different unsaved buffer. The dialog preview showed `new external revision`. Choosing the explicit Overwrite action wrote `overwrite from Koren` to disk and returned the document to a clean state. The document service's tests additionally cover external deletion, changed-again detection, reload revision increments, and refusal to silently save over newer contents. Rapid changes during a conflict and conflict while inspections run were not separately exercised in the GUI.

## Restart and unsaved-document behavior

Before restart, the theme was changed from OLED Black to Light. On reopening with the same isolated home directory, Light remained selected. The editor returned to its empty welcome state: the last workspace, open tabs, active tab, and running language server were not restored. No task, terminal, language server, or debugger was automatically started. The user must reopen a workspace and explicitly authorize language services again. Panel-dimension persistence was not verified in this interactive run.

Extension enablement/activation was not restored automatically; the earlier restart exercise required enabling and activating the example again. Extension settings and inspection profiles are stored separately in `~/.config/koren/extension-settings.json`; round-trip and legacy-format behavior have automated bridge tests. Persisting enabled/active process state is not part of the current restore behavior. Corrupted settings are treated as defaults by the loader, but preserving a damaged file and recovery from renamed or missing workspaces were not manually verified.

With a dirty Rust buffer, the native close request showed “Unsaved documents.” Cancel left the editor open and the modified marker intact. A later explicit Discard and close closed the application. No session recovery is implemented; dirty buffers are protected by this prompt rather than silently persisted.

## Final stabilization changes

The frontend now stores a versioned session record in WebKit local storage containing the workspace path, open workspace-relative tabs, active tab, and active standard panel. On startup it reopens the workspace and files in order, selects the prior active tab and panel, and reports files that no longer exist. It does not auto-start a terminal, debugger, language server, or extension. A missing workspace leaves the editor available and reports that the user must choose another. Unsupported or corrupt session records are ignored. Saved explorer and bottom-panel sizes are bounded to the current window at startup and resize; theme selection continues to use its existing independent preference. Corrupt, outdated-schema, unsafe-path, viewport-clamp, and multiple-definition-selection cases have frontend regression tests.

Session restart and missing-file recovery have not yet been manually repeated in WebKitGTK after these changes. The session record stores file paths, not unsaved document text: ordinary close still uses the dirty-document prompt, but an abrupt process or machine failure can lose unsaved edits. This remains a recovery limitation. Extension activation is not restored, so a failed activation cannot block session restoration; users must reactivate extensions explicitly.

Go to Definition now prompts when the language service returns multiple locations, ignores invalid selections, and reports an empty or unsupported result. Navigation retains the existing workspace boundary and opens the selected target through the document service, preserving an already-open target buffer. The multi-location selection helper is covered by frontend tests. Real same-file/cross-file rust-analyzer navigation, UTF-16 positioning, stale-result timing, and dirty-target behavior have not yet been completed in the desktop UI.

The installed rust-analyzer workflow still lacks a successful real quick-fix preview and confirmation. The earlier real session tested diagnostics, hover, and completion, and the bridge has a regression fixture for diagnostic-context forwarding, but this does not satisfy interactive quick-fix acceptance. These remaining interactive checks keep Phase 9.5 partial.

## Existing checks and remaining automated coverage

The repository already has document-service tests for conflict preview/reload/overwrite, a second external change, deleted files, and dirty close; settings tests cover legacy migration and profile round trips; frontend tests cover bounded theme/layout state helpers. The breakpoint path conversion has three focused frontend regression cases. LSP synchronization tests cover monotonically increasing document versions after close and reopen. The language-service bridge fixture verifies that diagnostic context and the quick-fix request filter reach the provider, and the protocol test verifies that old protocol-v2 requests without the optional context still deserialize.

Before this final stabilization change, `python3 scripts/check.py`, `cargo +1.85.0 check --workspace --locked`, and `python3 scripts/pr_check.py` passed. The baseline PR check included strict Clippy, all Rust unit and doc tests, Python tests (21 passed), all frontend tests (53 passed), typecheck, lint, production build, architecture checks, a clean workspace build, optimized editor build, and Linux desktop launch smoke. After the changes, `python3 scripts/pr_check.py` passed again, including 57 frontend tests and the clean and optimized builds; the WebKitGTK/Xvfb smoke reported 226.6 ms startup and 404,148 KiB process-tree RSS. The final standalone `scripts/check.py` and Rust 1.85 check are still pending.

## Performance boundaries

The three earlier no-tool desktop smoke samples remain the baseline: median visible window 174.2 ms and process-tree RSS 395,056 KiB at one second. The final PR smoke during this completion run reported 170.4 ms and 404,836 KiB. These are Linux/Xvfb smoke measurements, not interactive latency targets. A real rust-analyzer process was sampled at about 646 MiB RSS while indexing the small fixture, with two proc-macro helpers around 27 MiB each; Koren's process was about 204 MiB at that moment. These are single `ps` samples, not peak or steady-state measurements, and shared memory is not separated.

LSP initialization-to-ready time, edit-to-diagnostic latency, real debugger startup-to-first-stop, conflict-preview latency, restored-session latency, idle CPU with real services, and per-process WebKit memory were not instrumented. They remain unmeasured rather than inferred from the fake-server or launch-smoke benchmarks. See [BENCHMARKS.md](../BENCHMARKS.md) for the prior workload boundaries.

## Status

Phase 9.5 remains **partially complete**. Real GDB DAP, core rust-analyzer features, explicit conflict handling, theme persistence, and dirty-close behavior were exercised. Diagnostic context forwarding and stale-publication protection have regression coverage. Layout and workspace session restoration now have unit coverage, and the application still requires explicit user approval before starting privileged child processes. A real quick-fix preview and real Go-to-definition have not been verified. Restart recovery, abrupt-exit recovery of dirty buffers, and several settings edge cases also remain unverified. The required verification commands must all pass and the remaining interactive acceptance gaps must be resolved before another phase begins.
