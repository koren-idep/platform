# Phase 2 benchmark baseline

## Koren Code Editor desktop smoke sample

Build the Linux desktop app, then launch it in an X server (a virtual X server is suitable for CI):

```sh
cargo build --locked -p koren-code
python3 scripts/run_desktop_smoke.py target/debug/koren-code
```

The script waits until the visible “Koren Code Editor” window appears, reports elapsed process-start-to-window time, then samples total RSS across the editor process and its current descendants one second later. The application exits automatically in this test mode. One local sample on 2026-11-11 reported 170.2 ms to the window and 373,204 KiB process-tree RSS at the one-second sample point. Environment: Fedora Linux x86_64, kernel `7.2.9-310.t2.fc44.x86_64`, Intel Core i7-1068NG7 at 2.30 GHz, Xvfb plus Openbox, Rust 1.99.1, Tao 0.37.2, and Wry 0.57.2. This is one run, not a median; toolkit initialization, display server, and system load affect both values. Workspace-open, file-open, typing, and task-start interactive timings are not measured by this smoke script and remain uncharacterized. The measurements are diagnostics, not production-hardware performance claims.

This is a local baseline for comparison in later phases, not a performance target or a cross-machine claim. It was collected with:

```bash
cargo bench --locked -p koren-documents --bench phase2
```

## Environment

- Linux x86_64, kernel `7.2.9-310.t2.fc44.x86_64`
- Intel Core i7-1068NG7 CPU at 2.30 GHz, 8 online CPUs
- Rust `1.99.1` (`58cf8f9`, LLVM 23.1.1)
- Optimized Cargo bench profile
- Benchmark fixtures under `/tmp`, which was a `tmpfs`

## Results

One run from 2026-11-10. Times are arithmetic mean and minimum across the listed iterations.

| Workload | Fixture and operation | Iterations | Mean | Minimum |
|---|---|---:|---:|---:|
| Workspace open | Canonicalize and open root without scanning | 50 | 2.0 µs | 1.4 µs |
| Directory listing | Enumerate and sort 500 files in one directory | 50 | 744.8 µs | 552.2 µs |
| Text file open | Read and load a 1 MiB UTF-8 file | 20 | 1,107.6 µs | 969.8 µs |
| Small edit | Replace one character in an 8 MiB single-line document | 100 | 2.1 µs | 1.8 µs |
| Large edit | Replace 256 KiB in an 8 MiB single-line document | 10 | 114.1 µs | 96.7 µs |
| Modified save | Append one character and safely replace a 1 MiB file | 20 | 2,271.2 µs | 2,011.5 µs |

The save and file-open results are strongly affected by the in-memory temporary filesystem. Re-run on the target filesystem before using them to compare storage performance. The benchmark reports local elapsed times and does not measure tail latency, memory use, or crash durability.

## Phase 3 projects and tasks

Run with:

```sh
cargo bench --locked -p koren-projects --bench phase3
```

This non-gating custom benchmark registers 128 temporary-directory projects, enumerates them, registers task definitions, starts controlled instances of its own Rust executable, captures 32 KiB on each output stream, and starts four children concurrently. Task start dispatch includes the OS spawn call and output-reader thread creation. Task completion and output timings include process startup and exit; they are not pure Koren CPU costs.

Environment: Linux x86_64, kernel 7.2.9-310.t2.fc44.x86_64; Intel Core i7-1068NG7 at 2.30 GHz, 8 online CPUs; Rust 1.99.1 (58cf8f9), optimized Cargo bench profile; fixtures under the system temporary directory. One run on 2026-11-10:

| Workload | Iterations | Mean | Minimum |
|---|---:|---:|---:|
| Project registration | 128 | 10.7 µs | 5.1 µs |
| Project enumeration | 1,000 | 0.3 µs | 0.2 µs |
| Task definition registration | 100 | 0.8 µs | 0.4 µs |
| Task start dispatch, including OS spawn | 20 | 342.6 µs | 268.9 µs |
| Task completion, including child startup and exit | 20 | 10,118.8 µs | 10,088.7 µs |
| Direct process startup and exit baseline | 5 | 648.2 µs | 588.1 µs |
| Output capture, 32 KiB stdout and stderr | 5 | 59,051.2 µs | 17,482.1 µs |
| Four concurrent child completions | 4 | 11,187.0 µs | — |

The original approximately 10 ms completion floor came from the supervisor's 10 ms child-status polling interval. The output workload includes child creation, output production, pipe draining, and capture, and should not be read as an isolated byte-copy measurement. These values are local backend baselines only; rerun them on target hardware and storage before comparing.

## Phase 4 task completion remeasurement

Source inspection confirmed the supervisor called `Child::try_wait` every 10 ms. The process has a dedicated supervisor thread already, so the poll interval was reduced to 1 ms without blocking unrelated platform work or changing cancellation and timeout paths. One remeasurement on the same Linux machine and optimized profile produced:

| Workload | Original Phase 3 mean | After 1 ms polling | Notes |
|---|---:|---:|---|
| Task completion, including child startup and exit | 10,118.8 µs | 1,150.8 µs | About 88.6% lower; includes process startup and monitor scheduling |
| Four concurrent child completions | 11,187.0 µs | 2,427.7 µs | About 78.3% lower; includes process startup |
| Direct process startup and exit baseline | 648.2 µs | 627.9 µs | OS/process fixture baseline |
| Task start dispatch, including OS spawn | 342.6 µs | 235.1 µs | Includes pipe setup and reader-thread creation; not a pure Koren dispatch cost |
| Output capture, 32 KiB stdout and stderr | 59,051.2 µs | 49,747.6 µs | Includes child startup and scheduling; variation is too large to attribute to copying |

The completion change tracks the removed 10 ms polling floor. This is a single local run, not a cross-platform result. Output capture remains dominated by child startup, pipe scheduling, and fixture behavior; the benchmark does not isolate those costs or justify a claim about byte-copy performance. The 1 ms poll keeps bounded completion latency with a dedicated waiting thread, at the cost of more wakeups for long-running tasks.

## Phase 6 extension and language-service measurements

Run the reproducible local microbenchmarks with:

```sh
cargo bench --locked -p koren-lua --bench phase6
cargo bench --locked -p koren-lsp --bench phase6
```

One run on 2026-11-11, Linux x86_64, kernel `7.2.9-310.t2.fc44.x86_64`, Intel Core i7-1068NG7 at 2.30 GHz (8 CPUs), Rust `1.99.1`, Python `3.14.8`, optimized Cargo bench profile:

| Workload | Iterations | Mean | Minimum |
|---|---:|---:|---:|
| Empty extension-manager construction | 10,000 | 0.017 µs | 0.015 µs |
| Lua VM initialize, activate one command, then deactivate | 500 | 60.958 µs | 43.190 µs |
| Lua VM allocator bytes after activation | 1 | 19,443 bytes | — |
| Python fixture process start/exit baseline | 20 | 12,113.9 µs | 11,428.2 µs |
| Fake LSP process start, initialize, and shutdown | 20 | 75,985.0 µs | 72,841.1 µs |
| Fake LSP hover request/response | 100 | 107.9 µs | 68.0 µs |
| LSP document revision/change mapping | 10,000 | 0.317 µs | 0.259 µs |
| Fake LSP diagnostic publication after didOpen | 20 | 140.1 µs | — |

The LSP startup row includes the Python fixture process. The measured Python start/exit row is the external-process baseline; the difference is about 66.1 ms for LSP framing and initialization in this fixture. The Lua VM allocator figure is the VM's own allocated memory after activation, not total app RSS. These fixtures do not represent rust-analyzer or another production language server. The Phase 2 desktop smoke above is an application launch sample without an active language server. Phase 6.5 adds bridge-level save measurements with stalled initialization and hover, plus actor and idle-overhead measurements below.

## Phase 6.5 asynchronous language-service measurements

Run the actor benchmark and the bridge regression with:

```sh
cargo bench --locked -p koren-lsp --bench phase65
cargo test --locked -p koren-code stalled_lsp_initialization_and_hover_do_not_block_worker_save_or_close -- --nocapture
python3 scripts/run_desktop_smoke.py target/debug/koren-code
```

One local run on 2026-11-11, Linux x86_64, kernel `7.2.9-310.t2.fc44.x86_64`, Intel Core i7-1068NG7 at 2.30 GHz (8 CPUs), Rust `1.99.1`, Python `3.14.8`, optimized Cargo bench profile:

| Workload | Iterations/fixture | Result | What is included |
|---|---:|---:|---|
| LSP initialization dispatch | 4 independent actors | 58.6 µs mean, 51.8 µs minimum | Host validation, bounded-channel setup, and thread dispatch; excludes child startup and initialize response |
| Hover request dispatch | 4 sequential requests | 3.2 µs mean, 1.6 µs minimum | Actor mailbox enqueue and ticket creation; excludes transport and server response |
| Concurrent hover round trip | 4 independent fake servers | 0.349 ms | Actor scheduling, JSON-RPC, Python fixture work, and response delivery |
| Idle actor CPU | 4 actors over 2.001 s | 0 scheduler ticks | Sum of `/proc/self/task/*/stat` CPU ticks for the benchmark process's actor threads; tick resolution is 10 ms here, so this is below one tick, not proof of zero CPU use |
| Host RSS change after actor startup | 4 actors | +1,304 KiB | Benchmark process RSS only; does not include Python child process memory |
| Host RSS drift during 2 s idle sample | 4 actors | +8 KiB | Process RSS sample variation during the idle interval |
| Bridge initialize dispatch | 1 stalled fake server | 0.116 ms | Approve request response through the actual desktop backend worker; initialization remains gated |
| Bridge hover dispatch to fixture receipt | 1 stalled hover | 5.119 ms | Backend feature-pool scheduling, actor enqueue, pipe, and fake-server receipt; excludes hover response because the fixture remains gated |
| Save during stalled initialize | 1 save | 0.151 ms | Actual bridge save and safe filesystem write while initialize is blocked |
| Save during stalled hover | 1 save | 0.305 ms | Actual bridge save while the language feature request is blocked |
| Desktop window launch | 1 smoke run | 172.3 ms; 396,644 KiB process-tree RSS | Window appearance plus one-second RSS sample, with no active LSP process |

The LSP dispatch and round-trip rows are four-sample local microbenchmarks; the bridge rows are one deterministic gated regression run, not latency distributions. The fake servers run locally and do not model rust-analyzer's indexing or diagnostics load. The idle tick counter is Linux-specific and quantized; repeat with a longer interval and on target systems for tighter CPU bounds. The desktop launch smoke validates the virtual-display path only. On this host, `xvfb-run -a` failed because the installed T2 `xvfb-run` script uses a fixed three-second startup delay; a direct `xdpyinfo` probe still could not connect when the application started. The replacement launcher waits until Xvfb writes its `-displayfd` readiness signal, then runs with a fresh X11-only environment. The desktop smoke passed with that launcher. No Windows or macOS GUI environment was run.

## Phase 7 extension contribution measurements

Run the registry/Lua benchmark and the synthetic command-palette filter benchmark with:

```sh
cargo bench --locked -p koren-lua --bench phase7
node scripts/bench_phase7_palette.mjs
```

One local run on 2026-11-11, Linux x86_64, kernel `7.2.9-310.t2.fc44.x86_64`, Intel Core i7-1068NG7 at 2.30 GHz (8 CPUs), Rust `1.99.1`, Node `22.23.1`, optimized Cargo bench profile:

| Workload | Iterations/fixture | Result | What is included |
|---|---:|---:|---|
| Empty contribution registry initialization | 100,000 | 0.019 µs mean, 0.017 µs minimum | Empty owner map construction |
| Lua activation, register 65 commands, panel, setting, theme and diagnostic, registry commit, deactivate | 100 | 257.0 µs mean, 210.3 µs minimum | One in-process Lua VM and contribution transaction; not disk discovery or desktop rendering |
| Lua allocator after that activation | 1 | 36,828 bytes | Lua VM allocator only, not process RSS |
| Plain-text tool-window update and drain | 10,000 | 1.485 µs mean, 1.221 µs minimum | Lua callback, bounded queue insert, and host-side drain |
| Palette filter over 1,000 labeled commands | 10,000 | 120.4 µs mean, 90.7 µs minimum | Synthetic Node filter using the frontend's case-insensitive substring algorithm; excludes DOM rendering and event handling |
| Process RSS before/after one active extension | 1 | 4,256 KiB / 4,256 KiB (0 KiB sampled delta) | Immediate `/proc/self/status` samples in the benchmark process; 4 KiB granularity and allocator reuse make this too coarse to infer zero extension overhead |

Panel construction and multiple-command registration are measured as part of the activation transaction rather than as separate isolated timings. These are local microbenchmarks, not desktop interaction distributions. Windows and macOS performance were not measured.

## Phase 8 inspection measurements

Run the reproducible local inspection and Lua callback microbenchmarks with:

```sh
cargo bench --locked -p koren-inspections --bench phase8
cargo bench --locked -p koren-lua --bench phase8
cd apps/koren-code/frontend && node --experimental-strip-types scripts/bench_phase8_problems.mjs
```

One local run on 2026-11-11, Linux x86_64, kernel `7.2.9-310.t2.fc44.x86_64`, Intel Core i7-1068NG7 at 2.30 GHz (8 CPUs), Rust `1.99.1`, optimized Cargo bench profile:

| Workload | Iterations | Mean | Minimum |
|---|---:|---:|---:|
| Inspection profile lookup | 100,000 | 34.5 ns | 26 ns |
| Document inspection and finding ingestion | 500 | 239.3 µs | 196.5 µs |

The document fixture is 200 lines with one text marker per line and one provider executed in the bounded job thread. The measurement includes job dispatch, scanning, result construction, and collection. Additional bounded fixtures measured profile insert/select, explicit project analysis over 20 small documents (including result and latest progress retrieval), SARIF import/export for 200 findings, a Lua callback scanning 200 markers in one plain-text line, and frontend state publication plus the production Problems severity filter over 1,000 findings. One run on the same host and date:

| Workload | Iterations | Mean | Minimum | Fixture boundary |
|---|---:|---:|---:|---|
| Profile insert and workspace select | 10,000 | 226.4 ns | 184 ns | Two profiles and one workspace selection in a fresh in-memory profile set |
| Project progress and findings delivery | 100 | 368.6 µs | 315.4 µs | Bounded provider scans 20 documents, receives results, then drains the latest progress update; includes worker scheduling |
| SARIF export, 200 findings | 100 | 1.460 ms | 1.183 ms | Serialize supported SARIF fields and validate output size |
| SARIF import, 200 findings | 100 | 0.924 ms | 0.813 ms | Parse, validate paths/coordinates, and construct imported findings |
| Lua inspection callback, 200 markers | 500 | 2.932 ms | 2.127 ms | One 2,400-character ASCII line; Lua `string.find`, finding allocation, conversion, and runtime callback |
| Frontend diagnostic state publication, 1,000 findings | 10,000 | 1.029 µs | 0.078 µs | Assign the received finding array to the same Map-shaped cache used by the UI; excludes IPC decoding and DOM rendering |
| Problems severity filter, 1,000 findings | 10,000 | 12.956 µs | 2.077 µs | Calls production `filterInspectionFindings`; excludes DOM creation and browser paint |

The Rust figures are arithmetic means and minima from one process run. The frontend benchmark runs on Node `22.23.1` using `--experimental-strip-types`; it measures the production pure filter helper but only approximates the UI's cache write as publication. None of these measurements model desktop interaction latency, multiple extension providers, browser rendering, or large-workspace memory. They are local diagnostic baselines, not performance targets.

## Phase 9A workspace, search, Git, and results measurements

Reproduce the local backend workloads with:

```sh
cargo bench --locked -p koren-workspace --bench phase9a
cargo bench --locked -p koren-search --bench phase9a
cargo bench --locked -p koren-git --bench phase9a
npm run --prefix apps/koren-code/frontend benchmark:phase9a -- --silent=false
cargo build --locked --release -p koren-code
python3 scripts/run_desktop_smoke.py target/release/koren-code
```

One local run on 2026-11-11, Linux x86_64, kernel `7.2.9-310.t2.fc44.x86_64`, Intel Core i7-1068NG7 at 2.30 GHz, Rust `1.99.1`, Node `22.23.1`, optimized Rust benchmark profile:

| Workload | Fixture / iterations | Result | Measurement boundary |
|---|---:|---:|---|
| Create file plus rename | 500 operations | 17.1 µs mean, 13.5 µs minimum | Workspace API calls on the temporary filesystem |
| Create directory | 250 operations | 9.6 µs mean, 4.8 µs minimum | Workspace API call on the temporary filesystem |
| Text search | 1,000 small text files, 12 runs | 7.16 ms mean, 6.29 ms minimum | Full ignored-aware traversal and result delivery; about 100 matching lines |
| Search cancellation | Same tree, 12 runs | 0.22 ms mean, 0.15 ms minimum | Cancellation request through worker completion; does not include frontend poll delay |
| Repository discovery | 500 tracked files, 30 runs | 0.42 ms mean, 0.36 ms minimum | Explicit repository scan; no startup discovery |
| Git status | 500 tracked files, 50 runs | 1.23 ms mean, 1.05 ms minimum | libgit2 status collection and path conversion |
| One-file text diff | 50 runs | 1.36 ms mean, 1.14 ms minimum | Bounded libgit2 patch generation |
| Stage plus unstage | 50 runs | 1.60 ms mean, 1.31 ms minimum | One selected tracked file per operation pair |
| Search result grouping | 5,000 results / 100 files, 50 runs | 0.337 ms mean, 0.206 ms median | Production grouping helper; excludes DOM construction and browser paint |
| Desktop startup | 1 smoke run | 176.9 ms to window; 401,680 KiB process-tree RSS after 1 s | Debug executable, one local Linux/Xvfb run; same smoke timing and RSS boundary as above |

These fixtures are small local microbenchmarks on a temporary filesystem. They do not measure cold disks, network filesystems, very large repositories, UI paint, or concurrent editing. The Git dependency's native build/link size is platform/toolchain dependent. The startup result above is one local debug run; it measures process-start to window appearance and one-second process-tree RSS, not interactive latency. Windows and macOS were not measured.

## Phase 9B integrated terminal measurements

Controlled PTY measurements were recorded with `cargo test -p koren-terminal ... -- --nocapture` on the same Linux x86_64 development machine listed above. Each figure is a single local test sample using the Rust test executable as a controlled PTY child rather than the user's configured shell.

| Workload | Result | Measurement boundary |
|---|---:|---|
| Reserve a terminal session | 16.4 µs | Create the lazy session record; does not allocate a PTY |
| Allocate PTY and start controlled child | 0.69 ms | `start_session`, including PTY setup and process spawn |
| Input to echoed response | 10.13 ms | Input queue through observed echo; the test polls output every 10 ms, so this is a coarse upper-bound sample |
| Large output transfer | 2 MiB in 51.4 ms (~39 MiB/s) | Controlled child output drained in bounded 128 KiB batches; includes test-runner scheduling |

The PTY fixture verifies byte ordering and no loss across the measured 2 MiB payload. These values are not repeated-run statistics. Shell-specific startup time, per-session RSS split from child RSS, idle CPU, sustained concurrent sessions, xterm render responsiveness, and clipboard/terminal behavior in a native WebKitGTK window remain unmeasured. Desktop startup and one-second process-tree RSS are measured separately by the Linux smoke test; opening no terminal starts no shell or PTY worker.

After Phase 9B, the final Linux desktop smoke reported 171.1 ms to window appearance and 400,576 KiB process-tree RSS after one second, with no terminal opened. This is a single run using the same smoke boundary as the Phase 9A baseline above; it does not isolate terminal-service memory, and the RSS difference is within the variability of single-run samples.

## Phase 9C DAP measurements

Reproduce the deterministic fake-adapter workload with:

```sh
cargo bench --locked -p koren-debug --bench phase9c
```

One local run on 2026-11-11, Linux x86_64, optimized Rust bench profile, Python stdio fake adapter, 10 samples per workload:

| Workload | Mean / minimum | Measurement boundary |
|---|---:|---|
| Adapter spawn, DAP initialize, launch, and first paused state | 30.456 / 26.843 ms | Includes Python process startup, actor scheduling, initialization, launch, and polling until paused |
| Threads request/response | 0.038 / 0.009 ms | Host dispatch through fake DAP response observation |
| Stack trace request/response | 16.169 / 16.137 ms | Same boundary; test harness scheduling is included |
| Scopes request/response | 8.127 / 8.073 ms | Same boundary |
| Variables request/response | 8.089 / 8.074 ms | Same boundary |
| Second session start while first is active | 26.932 / 25.862 ms | Includes fake process startup and paused-state observation for session two |

These are single-process means/minima with a tiny local fixture, not percentiles or real-debugger measurements. The differing response times reflect polling and scheduling noise in this harness. Breakpoint synchronization latency, debug panel paint/interaction latency, idle CPU/RSS split from WebKit, and session event throughput under load were not separately measured. The editor startup smoke has no active debugger, so it shows lazy startup behavior only; it cannot isolate debug-service overhead. See [DEBUGGING.md](docs/DEBUGGING.md) for compatibility limits.
