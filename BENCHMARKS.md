# Phase 2 benchmark baseline

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
