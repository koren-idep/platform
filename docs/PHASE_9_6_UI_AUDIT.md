# Phase 9.6 UI audit and migration notes

## Current frontend surfaces

The desktop client is a TypeScript entry point (`apps/koren-code/frontend/src/main.ts`) with focused helpers under `src/app/`, CodeMirror modules under `src/editor/`, a typed Rust bridge in `apps/koren-code/src/bridge.rs`, and one shared stylesheet. The UI is assembled as a single workbench with a menu row, a tall bordered toolbar, a permanent project explorer, document tabs, a bottom tool panel, and a status bar. Search, Git, Problems, Tasks, Projects, Terminal, and Debug currently share the bottom panel. Extension tools are published through the existing typed contributions and are user-opened.

The settings surface is currently a small modal containing a theme selector, extension settings, and inspection profiles. Theme, layout, session, and recovery preferences use separate versioned or named WebKit local-storage entries. The Rust host atomically persists extension setting values and inspection profiles in the per-user Koren settings file. There is no general application-preferences protocol or workspace preferences file yet. Preserve the inspection/extension persistence contract and add new typed preferences without rewriting those services.

## Phase 9.6 starting point

- Keep Tao, Wry, CodeMirror 6, the typed protocol, and all existing platform services.
- Retain the current dark/light choice, but expose it through a centralized Koren token layer with dark near-black defaults.
- Preserve session/layout and recovery stores as separate transient state.
- Keep tool windows lazy; default a fresh profile to a collapsed bottom panel and a closed right panel.
- Do not add a framework, AI surface, Rust-specific editor affordance, or run-configuration engine.
- Build searchable settings from typed descriptors; mark unsupported settings explicitly instead of presenting inert controls.
- Keep extension settings and contributed tools on their existing host-owned APIs.

## Known migration risks

1. `main.ts` owns shell markup and many event routes. Replace it incrementally; broad file moves would make behavioral review and recovery regression checks harder.
2. Multiple preference records already exist in local storage. Migrate only values with explicit equivalence and preserve unknown extension settings.
3. Existing layout dimensions and tab/session restore are already tested. New defaults must not overwrite values that users have saved.
4. Terminal, debugger, inspections, and extension views are lazy or asynchronous. Layout changes must not start those services just by rendering their containers.
5. The current lower panel is part of the session schema. New sidebar selection and panel visibility need additive migration and must tolerate older schema-1 sessions.

## Revised plan after the course correction

### Milestone 9.6-A workbench review

The Fleet and IntelliJ reference screenshots supplied in the conversation were inspected directly. The requested repository copies under `design-references/` were not present in the checked working tree. No network retrieval was used. The Fleet reference guided the horizontal Files/Search/Git navigation, near-black chrome, compact top controls, separated rounded work areas, editor tabs, and terminal placement. IntelliJ informed the optional secondary tool panel and menu discoverability. The toolbar actions use original inline SVGs; no proprietary icon assets were copied.

The workbench now has a compact menu row and a dedicated action row with consistent new/open/save, undo/redo, source control, run, search, layout, and settings controls. The main canvas separates the horizontal project sidebar, editor tabs/editor, optional outline panel, bottom tool panel, and status bar into rounded dark surfaces. Existing editor, workspace, terminal panel, task, extension, and approval routes continue through the previous application services. The desktop capture is [phase-9-6-workbench.png](screenshots/phase-9-6-workbench.png) (1600×900, real WebKitGTK window, disposable workspace).

Compared with the Fleet reference, the broad hierarchy and panel geometry now align: Fleet-style horizontal sidebar tabs, a compact two-row header, a large central editor, separate bottom terminal area, and a secondary panel are visible. Remaining differences are deliberate or limited: Koren uses its own thin monochrome SVG set and blue active states; the screenshot's terminal panel is open but has no live shell session; the secondary panel currently shows the existing outline empty state. The Fleet screenshot's brighter syntax, denser editor content, and active terminal prompt are not part of this capture. This is the 9.6-A visual checkpoint, not final visual approval.

No new Settings work was undertaken as part of this milestone; the pre-existing in-progress Settings changes were preserved. A full Settings redesign, additional tool-window contributions, and later UI milestones remain outside 9.6-A. Preserve the new shell as the presentation layer over the existing typed bridge and application services.

Split editors, generalized docking, keybinding recording, and settings-file editing require additional document ownership and host APIs. They remain explicit follow-up items unless the staged implementation can support them without weakening shared document state or execution boundaries.
