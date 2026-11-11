# Architecture

## Crate responsibilities and dependency directions

The dependency graph points from consumers to the lower-level crates they use:

```text
koren-headless ──> koren-core, koren-protocol, koren-commands,
                   koren-workspace, koren-documents
koren-documents ─> koren-core, koren-protocol, koren-workspace, ropey
koren-workspace ─> koren-core
koren-protocol  ─> serde

koren-core      (no internal crate dependencies)
koren-commands  (no internal crate dependencies)
```

`koren-core` owns lifecycle primitives and the in-process event queue. `koren-protocol` owns typed versioned request/response contracts and JSON types. `koren-commands` owns command registration, enumeration, and invocation. `koren-workspace` opens one local root and provides bounded, demand-driven navigation. `koren-documents` manages revisioned text buffers and safe-ish local file saves. `koren-headless` is a consumer that wires these public crates together.

Cargo metadata architecture tests enforce internal dependency directions and reject frontend, language-specific, or agent-runtime dependencies in the backend crates.

## Workspace ownership and path policy

Each `WorkspaceManager` owns at most one open canonical root. Opening a second root before closing the first is an error; a failed open does not replace the existing root. Opening performs no directory scan. A directory listing visits one requested directory, sorts its entries, and defaults to a 10,000-entry limit. Rust applications can configure that limit. Entries include their relative path, type, size, and optional modification time.

Workspace-relative operations reject absolute paths, parent traversal, and every symlink component. Listings may report symlink entries, but cannot descend through them. This keeps ordinary navigation within the root for normal local use. Canonical containment is an additional point-in-time check; it is not a security sandbox against filesystem races or hostile concurrent local changes.

## Document lifecycle and text coordinates

`DocumentManager` owns independent in-memory documents. A document can be untitled or associated with one workspace file. Opening the same canonical file twice in one manager returns the same id, preventing competing writable buffers. Closing a dirty document requires explicit discard authorization.

Text is stored in [Ropey](https://docs.rs/ropey/latest/ropey/), a UTF-8 rope whose edits use Unicode scalar indices. This avoids copying an entire large buffer for each small edit and avoids invalid UTF-8 boundaries. Ropey reports logarithmic worst-case edit/query operations, including long single-line files; this is a library complexity statement, not a Koren performance claim. The platform exposes zero-based UTF-16 line/column positions for editor and future LSP interoperability and converts them to Ropey scalar indices. A UTF-16 position inside a surrogate pair, out-of-range line/column, or position inside a line terminator is rejected. LF and CRLF remain byte-for-byte in the buffer; positions exclude those terminator characters.

Revisions begin at zero. An edit batch validates every range against the same revision, rejects overlap and equal-start ambiguity, then applies edits from the end toward the beginning. A non-empty successful batch advances one revision. Expected-revision mismatches fail explicitly. Dirty state is revision-based: it clears only after a successful save or explicit reload.

## Loading and saves

The initial document limit is 16 MiB and is configurable per manager. Files are read as UTF-8; a leading UTF-8 BOM is removed from the buffer and restored on save. NUL-containing files and invalid UTF-8 are rejected, not decoded lossily. LF and CRLF are retained as loaded and are not normalized.

The manager records file size, modification time, and an in-process content fingerprint. A save checks the current file against that baseline. A mismatch returns a structured conflict without writing. The caller can reload the observed external version or explicitly overwrite it; overwrite is accepted only while the file still matches the version shown by the conflict. This is a conservative common-change detector, not an authorization boundary, and time-of-check/time-of-use races remain possible.

Saving writes a uniquely named temporary file in the destination directory, copies the existing permissions, flushes and synchronizes the temporary file, then renames it over the target. Rust's [`std::fs::rename`](https://doc.rust-lang.org/std/fs/fn.rename.html) platform behavior depends on Windows version and filesystem support. The containing directory is not synchronized, so power-loss durability is not promised. A failed save leaves the in-memory document dirty. Replacement behavior is currently verified on Linux; other operating systems need CI coverage.

## Event delivery

Workspace and document managers own their event queues. They enqueue lifecycle, change, save, and conflict events and expose a drain operation. Each manager's events are delivered to its consumer in FIFO order when drained. There is no global ordering across independent managers, persistence, broadcast, callback listener, or background thread. The consumer owns event processing and should drain promptly. Queue ownership avoids registered-listener lifetime leaks.

## Protocol evolution and headless session

The experimental protocol is version 2, adding workspace and document operations to the Phase 1 contract. Version 1 requests are rejected. Breaking changes should increment the version and update serialization and compatibility tests together. `koren-headless session --stdio` processes JSON Lines sequentially in one process, with caller-supplied request ids and typed responses. Machine-readable output goes to stdout; diagnostics go to stderr. The session is local-only, not an HTTP or network server.

## Frontend, products, and future interfaces

All crates remain frontend independent. Future desktop code is expected to use Tao for windows, Wry/WebKitGTK on Linux, and HTML/CSS/TypeScript outside these backend crates. Koren IDE and other products will be compiled Rust applications providing their own project models, language integrations, and build support. Lua 5.4 runtime extensions, WebAssembly, LSP, DAP, and agent interfaces are future plans only; none are in this workspace.

## Performance

`cargo bench --locked -p koren-documents --bench phase2` runs reproducible, non-gating workloads for workspace open, listing 500 entries, opening a 1 MiB text file, small and 256 KiB edits in an 8 MiB single-line document, and saving a modified document. Measurements and environment are recorded in [`BENCHMARKS.md`](BENCHMARKS.md). They are baselines for later comparisons, not targets or cross-machine claims.
