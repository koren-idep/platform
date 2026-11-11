#!/usr/bin/env python3
"""Run the complete required pull request validation suite."""
import subprocess
import sys


def run(*command):
    print("+", " ".join(command), flush=True)
    return subprocess.run(command).returncode


def main():
    commands = [
        ("cargo", "fmt", "--all", "--", "--check"),
        ("cargo", "clippy", "--workspace", "--all-targets", "--locked", "--", "-D", "warnings"),
        ("cargo", "test", "--workspace", "--locked"),
        ("cargo", "test", "--workspace", "--doc", "--locked"),
        (sys.executable, "scripts/architecture.py"),
        ("cargo", "build", "--locked", "-p", "koren-headless"),
        (sys.executable, "-m", "pytest", "-q"),
        ("cargo", "clean"),
        ("cargo", "build", "--workspace", "--locked"),
    ]
    for command in commands:
        if run(*command):
            return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
