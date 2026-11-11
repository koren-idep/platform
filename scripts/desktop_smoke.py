#!/usr/bin/env python3
"""Launch the Linux desktop app and report window startup time and process RSS."""
import json
import os
import pathlib
import subprocess
import sys
import time


def process_tree_rss_kib(root_pid):
    """Sum RSS for the app and its current descendants from procfs."""
    parents = {root_pid: None}
    for entry in pathlib.Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        try:
            fields = (entry / "stat").read_text().split()
            parents[int(entry.name)] = int(fields[3])
        except (OSError, ValueError, IndexError):
            continue
    descendants = {root_pid}
    changed = True
    while changed:
        changed = False
        for pid, parent in parents.items():
            if parent in descendants and pid not in descendants:
                descendants.add(pid)
                changed = True
    rss = 0
    for pid in descendants:
        try:
            for line in (pathlib.Path("/proc") / str(pid) / "status").read_text().splitlines():
                if line.startswith("VmRSS:"):
                    rss += int(line.split()[1])
                    break
        except (OSError, ValueError, IndexError):
            continue
    return rss


def main():
    if len(sys.argv) != 2:
        print(f"usage: {sys.argv[0]} PATH_TO_KOREN_CODE", file=sys.stderr)
        return 2
    environment = os.environ.copy()
    environment.pop("WAYLAND_DISPLAY", None)
    environment["GDK_BACKEND"] = "x11"
    environment["KOREN_CODE_SMOKE_EXIT_AFTER_MS"] = "5000"
    window_manager = subprocess.Popen(["openbox"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    process = None
    try:
        time.sleep(0.5)
        started = time.perf_counter()
        process = subprocess.Popen([sys.argv[1]], env=environment)
        deadline = started + 20
        while time.perf_counter() < deadline:
            if process.poll() is not None:
                raise RuntimeError(f"Koren Code Editor exited early with status {process.returncode}")
            found = subprocess.run(
                ["xdotool", "search", "--onlyvisible", "--name", "^Koren Code Editor$"],
                check=False,
                capture_output=True,
                text=True,
            )
            if found.returncode == 0 and found.stdout.strip():
                break
            time.sleep(0.05)
        else:
            raise RuntimeError("Koren Code Editor window did not appear within 20 seconds")

        time.sleep(1)
        print(json.dumps({
            "window_startup_ms": round((time.perf_counter() - started - 1) * 1000, 1),
            "process_tree_rss_kib": process_tree_rss_kib(process.pid),
            "sample_delay_after_window_seconds": 1,
        }), flush=True)
        status = process.wait(timeout=15)
        if status:
            raise RuntimeError(f"Koren Code Editor exited with status {status}")
        return 0
    finally:
        if process is not None and process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
        window_manager.terminate()
        try:
            window_manager.wait(timeout=2)
        except subprocess.TimeoutExpired:
            window_manager.kill()
            window_manager.wait()


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, RuntimeError, subprocess.TimeoutExpired) as error:
        print(f"desktop smoke failed: {error}", file=sys.stderr)
        raise SystemExit(1)
