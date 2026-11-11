#!/usr/bin/env python3
"""Start the Linux desktop smoke in a clean X11 session with a free Xvfb display."""
import os
import select
import subprocess
import sys


def stop(process):
    if process is None or process.poll() is not None:
        return
    process.terminate()
    try:
        process.wait(timeout=3)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait()


def main():
    if len(sys.argv) != 2:
        print(f"usage: {sys.argv[0]} PATH_TO_KOREN_CODE", file=sys.stderr)
        return 2
    environment = os.environ.copy()
    for name in ("DISPLAY", "WAYLAND_DISPLAY", "XAUTHORITY", "GDK_BACKEND"):
        environment.pop(name, None)
    server = subprocess.Popen(
        ["Xvfb", "-displayfd", "1", "-screen", "0", "1280x800x24", "-nolisten", "tcp"],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        env=environment,
    )
    try:
        ready, _, _ = select.select([server.stdout], [], [], 5)
        if not ready:
            stop(server)
            details = server.stderr.read().strip()
            raise RuntimeError(
                "Xvfb did not allocate a display within 5 seconds"
                + (f": {details}" if details else "")
            )
        display_number = server.stdout.readline().strip()
        if not display_number.isdigit():
            stop(server)
            details = server.stderr.read().strip()
            raise RuntimeError(f"Xvfb returned an invalid display number: {details}")
        environment["DISPLAY"] = f":{display_number}"
        environment["GDK_BACKEND"] = "x11"
        return subprocess.run(
            [sys.executable, "scripts/desktop_smoke.py", sys.argv[1]],
            env=environment,
            check=False,
        ).returncode
    finally:
        stop(server)
        if server.stderr:
            server.stderr.close()
        if server.stdout:
            server.stdout.close()


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, RuntimeError, subprocess.TimeoutExpired) as error:
        print(f"desktop smoke launcher failed: {error}", file=sys.stderr)
        raise SystemExit(1)
