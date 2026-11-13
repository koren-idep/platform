# Phase 9.6-A Visual and Project Discovery Review

## Cargo discovery correction

The previous toolbar selector initially showed **Discover Cargo…**. Pressing Run with no selected task called `discover_cargo_projects` with `max_directories: 6000`, `max_projects: 256`, and `max_depth: 32`. The frontend then called `list_tasks`, populated the run selector, rendered Cargo-specific project summaries, and opened Tasks. The Rust bridge called `CargoProvider::discover`, then registered that provider's task definitions. Discovery itself parsed filesystem manifests; it did not run Cargo or project code.

That selector and Cargo-specific menu item are no longer visible. A new `discover_registered_projects` operation asks the project manager to scan each registered provider with bounded limits. Provider task definitions are registered with the existing task manager, and each task remains subject to the normal task execution approval flow.

Workspace opening performs the scan automatically. **Refresh Project** in the Projects tool window and View menu repeats the same scan. A live desktop check opened the workspace, saw its Cargo project and tasks, added a second Cargo manifest while the app stayed open, then used Refresh Project and saw the second project without restarting. The backend regression test also verifies repeated scans do not duplicate projects or tasks and that task execution still requires approval.

## Screenshot comparison

The updated real WebKitGTK captures are:

- [Workbench default, 1600×900](screenshots/phase-9-6/10-workbench-default.png)
- [Projects discovered on workspace open, 1600×900](screenshots/phase-9-6/11-project-discovery.png)
- [Provider-supplied tasks, 1600×900](screenshots/phase-9-6/12-provider-tasks.png)
- [Settings opened as a main-view tab, 1600×900](screenshots/phase-9-6/13-settings-main-view-tab.png)
- [Switching between a file and the Settings tab, 1600×900](screenshots/phase-9-6/14-editor-settings-tab-switch.png)
- [Blue menu gradient, 1600×900](screenshots/phase-9-6/15-blue-menu-gradient.png)
- [Violet menu gradient, 1600×900](screenshots/phase-9-6/16-violet-menu-gradient.png)
- [Project colors disabled, 1600×900](screenshots/phase-9-6/17-project-colors-disabled-gradient.png)
- [Narrow menu overflow header crop, 720 px](screenshots/phase-9-6/18-narrow-menu-overflow.png)

The workbench follows Fleet's horizontal Files/Search/Git navigation, compact editor tabs, dark separated surfaces, and bottom tool area. Its default project pane is 380 px wide at 1600×900, closer to the Fleet reference's sidebar proportions than the previous 300 px default. It keeps direct menu labels and native window actions in the UI title row, without the blue code mark, Koren wordmark, or waffle button. Project color fades from the workspace selector across the menu labels and reaches the standard header background by Help; the flexible space and right-side actions remain uniform. Settings now opens as a selectable tab inside the editor viewer and coexists with open documents. The universal run selector remains reserved for a later phase. The current project view is intentionally provider-neutral, while task names may identify the provider that supplied them.

## Verification

- Frontend typecheck and test suite: 89 tests passed.
- `koren-projects` provider-wide detection and repeat-refresh test passed.
- Desktop bridge refresh test passed for Cargo detection, stable repeated results, task registration, and execution approval.
- `scripts/run_desktop_smoke.py`: 175.8 ms to window and 432,116 KiB sampled process-tree RSS. The earlier local baseline was 172.4 ms and 415,236 KiB.
- Manual WebKitGTK check opened Settings, switched to the already-open Cargo manifest, and returned to Settings using the tab strip. The settings panel did not cover or replace the workbench.
- Captured the project-color states and narrow menu overflow in the running WebKitGTK app. Afterward, the disposable workspace identity was restored to its automatically assigned color and Project Colors was re-enabled.
- The app was launched against `/tmp/koren-phase96-v2-fixture`; no Cargo task was started during discovery.

The automated UI still uses WebKitGTK's profile storage for editor preferences. Universal Run/Debug configurations remain outside this correction, and no Phase 9.7 work was started.
