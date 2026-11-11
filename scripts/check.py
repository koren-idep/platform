#!/usr/bin/env python3
"""Run quick local formatting, compile, and architecture checks."""
import subprocess
import sys


def run(*command):
    print("+", " ".join(command), flush=True)
    return subprocess.run(command).returncode


def main():
    commands = [
        ("cargo", "fmt", "--all", "--", "--check"),
        ("cargo", "check", "--workspace", "--locked"),
        (sys.executable, "scripts/architecture.py"),
        ("npm", "ci", "--prefix", "apps/koren-code/frontend"),
        ("npm", "run", "--prefix", "apps/koren-code/frontend", "typecheck"),
        ("npm", "run", "--prefix", "apps/koren-code/frontend", "lint"),
        ("npm", "test", "--prefix", "apps/koren-code/frontend"),
        ("npm", "run", "--prefix", "apps/koren-code/frontend", "build"),
    ]
    for command in commands:
        if run(*command):
            return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
