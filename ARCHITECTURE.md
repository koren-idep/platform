# Architecture

## Crate responsibilities and dependency directions

The dependency graph points from consumers to the lower-level crates they use:

```text
koren-headless ──> koren-core, koren-protocol, koren-commands,
                   koren-workspace, koren-documents, koren-projects,
                   koren-tasks, koren-cargo
koren-code ──────> platform crates plus koren-extensions, koren-lua,
                   koren-inspections, koren-lsp
koren-documents ─> koren-core, koren-protocol, koren-workspace, ropey
koren-projects ──> koren-core, koren-workspace, koren-tasks
koren-tasks ─────> koren-core
koren-cargo ─────> koren-projects, koren-tasks, koren-workspace,
                   serde, serde_json, toml, glob
koren-extensions (no internal dependencies)
koren-lsp ───────> koren-documents, koren-protocol
koren-inspections -> koren-protocol
koren-lua ───────> koren-extensions, koren-inspections, koren-protocol,
                   mlua (Lua 5.4)
koren-workspace ─> koren-core
koren-protocol  ─> serde

koren-inspections owns provider-neutral finding contracts, typed profile overrides, bounded explicit provider jobs, and a bounded SARIF 2.1.0 adapter. Hosts remain responsible for document freshness checks and safe edit application.

koren-core      (no internal crate dependencies)
koren-commands  (no internal crate dependencies)
```

Koren core owns lifecycle primitives and shared event vocabulary. Koren protocol owns typed versioned request/response contracts and JSON types. Koren commands owns command registration, enumeration, and invocation. Koren workspace opens one local root and provides bounded, demand-driven navigation. Koren documents manages revisioned text buffers and local file saves. Koren projects owns manager-local logical projects and explicit bounded discovery providers; providers may expose generic task definitions. Koren tasks owns those definitions, host authorization checks, direct-process execution, bounded output, and run lifecycle. Koren cargo is an optional first-party Rust integration and is not a dependency of the generic project, task, workspace, or document crates. `koren-extensions` owns versioned manifests, lifecycle, and typed contribution registrations. `koren-lua` adapts Lua 5.4 callbacks to those shared contributions with a restricted standard-library set and VM limits. `koren-lsp` owns bounded JSON-RPC framing, direct stdio process configuration, typed language results, diagnostics, and revision/version synchronization. The desktop app composes these host-independent platform services.

## Project model and discovery

A workspace is a filesystem environment. A project is a logical unit registered within a workspace and identified by a ProjectId that is stable only for one manager instance. It carries a display name, workspace-relative root, provider ID, optional configuration path, provider data, and lifecycle state. Each workspace-relative root can be registered once, even when multiple providers recognize it. Nested roots are allowed. Closing a workspace removes its project records.

ProjectProvider inspects one directory's already bounded workspace entries and returns an optional candidate. ProjectManager discovery is explicit; limits bound directories, nesting, and returned projects. No provider command or configuration file is run. MarkerFileProvider only recognizes a configured filename and exists as a small fixture.

`koren-cargo` reads up to 1 MiB from each explicitly inspected Cargo manifest and honors workspace member and exclude globs. One workspace root is one Koren project; its package is in the root package field when present, while member packages are typed metadata under that workspace project. A nested Cargo workspace is a separate project, and a non-member package manifest remains an independent project. Discovery is bounded and performs no Cargo invocation. Manifest-declared names, directly declared versions, and common targets are available immediately. An explicit `cargo metadata --offline --format-version 1 --no-deps` task can provide authoritative inherited versions and target paths; its JSON output is parsed through a typed adapter. The task uses the regular host approval path and never runs during discovery.

The provider contributes workspace-level build/check/test/clean tasks and package-scoped build/check/test/clean plus binary run tasks where targets are known. Each uses executable `cargo`, an argv vector, an explicit manifest path, and the project workspace as its working directory. Cargo still reads the project's normal configuration. There is no implicit shell or Cargo invocation in generic services.

Providers return zero or more TaskDefinitions to describe supported capabilities. Definitions use a key scoped to their project, a display name/category, executable plus argv, working directory, environment overrides, optional timeout, and optional project ID. Koren tasks depends only on Koren core; the project crate depends on task contracts, avoiding a cycle.

## Task execution and authorization

TaskManager starts each process with std::process::Command, never an implicit shell. A supervisor thread monitors each child while two reader threads drain stdout/stderr, allowing host request handling to remain responsive. Process starts are synchronous and include the OS spawn call; subsequent waiting and output collection are asynchronous. The default service allows four concurrent children and retains up to 1 MiB for each stream. It continues draining after truncation. Task event and shared platform event queues are FIFO within the service and bounded; oldest events are discarded at capacity. Independent managers have no cross-service ordering.

Run states are Pending, Running, Succeeded, Failed, Cancelled, TimedOut, and SpawnFailed. A spawned process's nonzero exit is Failed with its exit code. An OS spawn error is SpawnFailed with the error message. Cancellation and timeout kill the direct child. After child exit, the supervisor waits up to 200 ms for both output readers to drain; if a descendant inherited a pipe, it completes the run and stops retaining later output, while that reader thread may remain until the pipe closes. Process group/job management is deliberately not implemented, so descendants may remain alive. Workspace close and task manager drop request cancellation of all active direct children; the host should keep the manager alive until task completion when it needs final output.

Registering a task grants no execution permission. A host supplies a TaskAuthorizer for immediate starts and a TaskExecutionPolicy for requests that can be allowed, denied, or held for approval. TaskApprovalManager stores a manager-local one-time request ID and immutable command snapshot, including actor and environment overrides. Approval consumes the request before starting and verifies the registered definition still matches the snapshot. Pending requests do not consume process concurrency slots, expire after their host-set lifetime, and can be invalidated on project, workspace, or session closure. Headless request operations default to pending approval; explicit approve/deny operations consume the request. The legacy --allow-task-execution flag remains a deliberately permissive trusted-host mode and is never enabled by default.

Protocol approval summaries include the executable, arguments, working directory, project, actor, and environment variable names, but never values. Rust hosts should keep environment values out of logs and presentation surfaces. Approvals are not sandboxing: a permitted Cargo build can execute project-controlled build scripts and programs. Workspace path checks do not constrain a running process.

Cargo metadata architecture tests enforce internal dependency directions and reject frontend, language-specific, or agent-runtime dependencies in the backend crates. Only `koren-lua` may depend on `mlua`; generic crates do not depend on the desktop app.

## Workspace ownership and path policy

Each `WorkspaceManager` owns at most one open canonical root. Opening a second root before closing the first is an error; a failed open does not replace the existing root. Opening performs no directory scan. A directory listing visits one requested directory, sorts its entries, and defaults to a 10,000-entry limit. Rust applications can configure that limit. Entries include their relative path, type, size, and optional modification time.

Workspace-relative operations reject absolute paths, parent traversal, and every symlink component. Listings may report symlink entries, but cannot descend through them. This keeps ordinary navigation within the root for normal local use. Canonical containment is an additional point-in-time check; it is not a security sandbox against filesystem races or hostile concurrent local changes.

## Document lifecycle and text coordinates

`DocumentManager` owns independent in-memory documents. A document can be untitled or associated with one workspace file. Opening the same canonical file twice in one manager returns the same id, preventing competing writable buffers. Closing a dirty document requires explicit discard authorization.

Text is stored in [Ropey](https://docs.rs/ropey/latest/ropey/), a UTF-8 rope whose edits use Unicode scalar indices. This avoids copying an entire large buffer for each small edit and avoids invalid UTF-8 boundaries. Ropey reports logarithmic worst-case edit/query operations, including long single-line files; this is a library complexity statement, not a Koren performance claim. The platform exposes zero-based UTF-16 line/column positions for editor and LSP interoperability and converts them to Ropey scalar indices. A UTF-16 position inside a surrogate pair, out-of-range line/column, or position inside a line terminator is rejected. LF and CRLF remain byte-for-byte in the buffer; positions exclude those terminator characters.

Revisions begin at zero. An edit batch validates every range against the same revision, rejects overlap and equal-start ambiguity, then applies edits from the end toward the beginning. A non-empty successful batch advances one revision. Expected-revision mismatches fail explicitly. Dirty state is revision-based: it clears only after a successful save or explicit reload.

## Loading and saves

The initial document limit is 16 MiB and is configurable per manager. Files are read as UTF-8; a leading UTF-8 BOM is removed from the buffer and restored on save. NUL-containing files and invalid UTF-8 are rejected, not decoded lossily. LF and CRLF are retained as loaded and are not normalized.

The manager records file size, modification time, and an in-process content fingerprint. A save checks the current file against that baseline. A mismatch returns a structured conflict without writing. The caller can reload the observed external version or explicitly overwrite it; overwrite is accepted only while the file still matches the version shown by the conflict. This is a conservative common-change detector, not an authorization boundary, and time-of-check/time-of-use races remain possible.

Saving writes a uniquely named temporary file in the destination directory, copies the existing permissions, flushes and synchronizes the temporary file, then renames it over the target. Rust's [`std::fs::rename`](https://doc.rust-lang.org/std/fs/fn.rename.html) platform behavior depends on Windows version and filesystem support. The containing directory is not synchronized, so power-loss durability is not promised. A failed save leaves the in-memory document dirty. Replacement behavior is currently verified on Linux; other operating systems need CI coverage.

## Event delivery

Workspace and document managers own their event queues. They enqueue lifecycle, change, save, and conflict events and expose a drain operation. Project events retain at most 1,024 entries; task and shared task events use the configured task event limit (4,096 by default). Each service is FIFO; when its queue fills, it drops the oldest event. There is no global ordering across independent managers, persistence, broadcast, callback listener, or background thread. The consumer owns event processing and should drain promptly. Queue ownership avoids registered-listener lifetime leaks.

## Protocol evolution and headless session

The experimental protocol remains version 2. Project, Cargo, task, and approval request/response variants are additive tagged operations; existing version-2 shapes are unchanged. Version 1 requests are rejected. Any incompatible field or encoding change must increment the version and update serialization and compatibility tests together. The headless session processes JSON Lines sequentially in one process, with caller-supplied request IDs and typed responses. Machine-readable output goes to stdout; diagnostics go to stderr. The session is local-only, not an HTTP or network server.

## Frontend, products, and future interfaces

All platform crates remain frontend independent. Koren Code Editor is the language-neutral desktop consumer in `apps/koren-code`: a Rust Tao/Wry host, a TypeScript frontend, and CodeMirror 6. Linux uses Wry's GTK container and WebKitGTK 4.1. The frontend accesses services through protocol-v2 JSON requests on the same-origin `koren://app` scheme. The native host serves only embedded or local built assets, rejects external navigation, applies a restrictive content security policy, and exposes a narrow IPC message for the dirty-close decision. Other application requests remain typed versioned protocol messages handled on one backend worker thread. Request bodies are limited to 1 MiB; event delivery uses bounded service queues and a reconnecting long poll.

The editor owns generic file navigation, document tabs, text editing, themes, task presentation, and approval dialogs. Language-specific editor contributions are registered through the frontend `LanguageIntegration` seam; Rust syntax support is one initial integration. Cargo project discovery and tasks come from the reusable `koren-cargo` platform provider. No Rust-only project navigation, settings, or editor assumptions belong in the universal shell. Task approval is a user decision, not a sandbox; the UI shows the exact executable, arguments, working directory, project, and environment variable names before approval.

The product split is: **Koren IDE Platform**, the reusable language-neutral backend and extension framework; **Koren Code Editor**, the universal lightweight editor; and **Koren IDE**, a future Rust-specialized distribution using the same desktop shell and platform APIs. Phase 6 adds local extension management and an LSP client integrated with document synchronization, diagnostics, hover, completion, and definition navigation. Phase 6.5 gives each authorized LSP process an independent actor thread and bounded FIFO mailbox. Initialization, transport I/O, feature waits, and process shutdown run outside the shared platform worker. A fixed four-thread feature pool and bounded queue keep frontend IPC callbacks off the GTK/WebView thread; server notifications use each actor's ordered queue. `DocumentManager` remains the sole owner of text and revisions. Feature responses carry the requested document revision, and closing a document cancels or invalidates its pending results. Queue overflow retires the affected server instead of silently dropping a document notification. Phase 7 adds a manager-local owner-indexed `ContributionRegistry`: activation stages a complete contribution set, validates it, then publishes atomically; deactivation removes that owner's set. Phase 8 adds `koren-inspections`, a language-neutral contract for provider-attributed findings, retained profiles, generic suppressions, explicit bounded document/project jobs, and bounded SARIF 2.1 import/export. Providers receive immutable document snapshots and cannot walk the filesystem or start tools through this API. The LSP actor supports code actions, intentions, references, rename proposals, and formatting. Desktop previews use revision-checked single-document edits; multi-file and resource-operation changes are rejected or labeled unsupported. The editor exposes these features through shortcuts and confirmation previews. Lua inspection registration and desktop SARIF import/export commands remain follow-up work. The frontend renders only host components, uses `textContent`, and maps approved design tokens to CSS variables. Project wizard forms declare fields only; host-authorized filesystem/task work remains outside extension UI permission. Extensions share the same manifest, Lua, and LSP APIs across products.

## Performance

Cargo bench for Koren documents runs workspace/document workloads. Cargo bench for Koren tasks covers project registration/enumeration, task registration/start, output capture, and concurrent children. Source inspection found task completion polling every 10 ms; the supervisor interval is now 1 ms. The same Linux benchmark dropped mean task completion from about 10.1 ms to 1.15 ms and four-child completion from 11.2 ms to 2.43 ms. Output capture remains dominated by child startup and scheduling. Phase 6.5 benchmarks measure actor dispatch separately from fake-server work, parallel fake-server requests, idle actor CPU ticks/RSS, and actual bridge saves while initialization or hover is stalled. Phase 8 measures inspection-profile lookup and finding ingestion against a fixed 200-line fixture. Measurements and environment are recorded in BENCHMARKS.md; they are local baselines, not full IDE performance claims.

## Verification

The minimum supported compiler is Rust 1.85. CI has a direct 1.85 build/test job, plus platform build/unit/integration jobs. The complete Linux PR suite includes frontend typecheck, lint, unit tests, production bundle, Rust checks, Python integration, a clean workspace build, release editor build, and an Xvfb window launch smoke test. The smoke launcher uses Xvfb's display-ready file descriptor rather than a fixed delay or display number. Cross-platform GUI availability still requires successful CI on the relevant native runners. The desktop smoke script reports the interval until the named window appears and samples the editor process tree's RSS one second later; this is a repeatable local/CI diagnostic, not a user-perceived performance target.
