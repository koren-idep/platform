# Phase 9.6 Workbench and Settings Audit

## Workbench shell

The frontend remains a TypeScript workbench over the existing typed Rust bridge, Tao/Wry host, and CodeMirror 6 editor. The shell has a compact direct menu bar, horizontal Files/Search/Git/Extensions sidebar tabs, editor tabs, a resizable bottom tool area, an optional outline panel, and a minimal status bar. The title area has custom minimize, maximize/restore, and close controls. The visible product wordmark and waffle menu are removed; the menu labels remain directly accessible. Run configuration selection is reserved for a later phase.

The dark workbench uses near-black chrome, restrained borders, compact controls, rounded panel surfaces, and original inline SVG icons. The project-color wash spans the project selector and menu labels, then fades to the normal header background at Help; the flexible gap and utility controls stay uniform. The header row and body now share the same 44 px alignment, and the status bar follows the panel corner radius and horizontal inset. The default project pane is 380 px wide at 1600×900. The supplied Fleet screenshot is the primary layout reference; the IntelliJ screenshot informs the secondary tooling and discoverability. The app keeps Koren's own icons and hierarchy. It does not copy Fleet assets or add its AI surface.

## Workspace project discovery

Opening a workspace triggers `discover_registered_projects` with fixed limits of 6,000 directories, 256 project candidates, and depth 32. The Rust project manager calls all registered providers; the frontend does not select or name Cargo. Provider discovery inspects workspace filesystem entries and manifests only. It does not execute Cargo, project commands, or project code, and it does not fetch dependencies. The Cargo provider remains registered and continues to provide its existing task definitions.

Detected projects are loaded into the Projects tool window and provider tasks into Tasks. The menu and Projects panel expose **Refresh Project** for a bounded rescan. Discovery failures are shown in the Projects and Tasks panels and in the status message; workspace opening remains available. Tasks still go through the existing task authorization flow. The legacy Cargo-specific protocol operation remains for compatibility, but the main UI no longer exposes it.

## Settings and preferences

Settings are built from typed descriptors and support search, user/workspace scope, validation, reset, and live theme/editor changes. Opening Settings adds a selectable Settings tab beside document tabs in the main editor viewer; it no longer replaces or overlays the whole workbench. Current controls cover theme, menu visibility, project identity and accents, density, toolbar visibility, editor typography, indentation, line numbers, and wrapping. The settings and keybinding stores are browser-profile local storage; there is not yet a Rust settings-file editor. Extension settings and inspection profiles continue to use their existing host-owned persistence APIs.

## Visual review

Real WebKitGTK captures at 1600×900 are saved in [the Phase 9.6 screenshots](screenshots/phase-9-6/). Captures 13 and 14 show Settings as a main-view tab and verify switching between it and a real open document. Compared with the Fleet reference, the hierarchy matches its horizontal sidebar tabs, dark separated work areas, compact editor tabs, and bottom tools. Koren's title area keeps its visible menu labels and custom window buttons; its panel proportions and original icon set differ from Fleet. The IntelliJ reference supports opening Projects and Tasks from the View menu rather than adding a permanent activity rail.

## Scope

The Settings implementation is still evolving. Generalized docking, settings-file editing, and universal Run/Debug configurations are not included here. No Phase 9.7 work is part of this checkpoint.
