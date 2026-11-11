"""Dependency boundary validation for the Koren Cargo workspace."""
import argparse
import json
import subprocess
import sys

ALLOWED_INTERNAL_DEPS = {
    "koren-core": set(),
    "koren-protocol": set(),
    "koren-commands": {"koren-core", "koren-protocol"},
    "koren-workspace": {"koren-core", "koren-protocol"},
    "koren-headless": {"koren-core", "koren-protocol", "koren-commands", "koren-workspace"},
}


def violations(metadata):
    """Return internal dependency edges that violate the documented graph."""
    names = set(ALLOWED_INTERNAL_DEPS)
    result = []
    for package in metadata.get("packages", []):
        source = package["name"]
        if source not in names:
            continue
        allowed = ALLOWED_INTERNAL_DEPS[source]
        for dependency in package.get("dependencies", []):
            target = dependency["name"]
            if target in names and target not in allowed:
                result.append(f"{source} -> {target}")
    return sorted(result)


def cargo_metadata():
    completed = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--no-deps"],
        check=True, capture_output=True, text=True,
    )
    return json.loads(completed.stdout)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--metadata-file", help="validate a saved Cargo metadata document")
    args = parser.parse_args()
    try:
        if args.metadata_file:
            with open(args.metadata_file, encoding="utf-8") as source:
                metadata = json.load(source)
        else:
            metadata = cargo_metadata()
        errors = violations(metadata)
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"architecture check failed: {error}", file=sys.stderr)
        return 1
    if errors:
        print("forbidden crate dependency direction(s): " + ", ".join(errors), file=sys.stderr)
        return 1
    print("architecture boundaries passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
