"""Pass validated package selections to Cargo and archived nextest without a shell."""

import argparse
import json
import os
from pathlib import Path
import re
import subprocess


def packages(raw):
    value = json.loads(raw)
    if not isinstance(value, list) or not value:
        raise ValueError("Rust jobs require a nonempty package list")
    if value != ["*"] and any(not isinstance(p, str) or not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_-]*", p) for p in value):
        raise ValueError("invalid package selection")
    return value


def command(action, names, shard=None):
    flags = ["--workspace"] if names == ["*"] else [arg for p in names for arg in ("-p", p)]
    if action == "tests":
        if shard not in {1, 2, 3}:
            raise ValueError("invalid shard")
        selection = "all()" if names == ["*"] else " or ".join(f"package(={p})" for p in names)
        return ["bash", "../.github/scripts/backend-nextest.sh", "run", "--no-fail-fast",
                "-E", f"({selection}) and not (binary(=media_image_peak) or binary(=turn_metrics))",
                "--partition", f"count:{shard}/3"]
    commands = {
        "lint": ["cargo", "clippy", *flags, "--all-targets", "--all-features", "--locked", "--", "-D", "warnings"],
        "build": ["cargo", "build", *flags, "--locked"],
        "doc": ["cargo", "test", *flags, "--doc", "--locked"],
    }
    return commands[action]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("action", choices=["lint", "build", "doc", "tests"])
    parser.add_argument("--shard", type=int)
    args = parser.parse_args()
    names = packages(os.environ["TRACK_PACKAGES"])
    print(f"Rust {args.action}: {', '.join(names)}", flush=True)
    root = Path(__file__).resolve().parents[3]
    subprocess.run(command(args.action, names, args.shard), cwd=root / "api", check=True)


if __name__ == "__main__":
    main()
