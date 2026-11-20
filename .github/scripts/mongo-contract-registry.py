"""Load independent Mongo suite registrations for the runner and its tests."""

import argparse
import json
from pathlib import Path
import re


SUITES = Path(__file__).with_name("mongo-contract-suites")
PREFIX = re.compile(r"(?:[A-Za-z_][A-Za-z0-9_]*::)+")


def load_prefixes(directory=SUITES):
    owners = {}
    for path in sorted(Path(directory).iterdir()):
        if path.is_symlink() or not path.is_file() or path.suffix != ".json":
            raise ValueError(f"Expected a regular suite JSON file: {path.name}")
        with path.open() as source:
            try:
                entry = json.load(source)
            except json.JSONDecodeError as error:
                raise ValueError(f"Invalid JSON in {path.name}: {error}") from error
        if not isinstance(entry, dict) or set(entry) != {"prefix"}:
            raise ValueError(f"Suite registration needs only a prefix: {path.name}")
        prefix = entry["prefix"]
        if not isinstance(prefix, str) or PREFIX.fullmatch(prefix) is None:
            raise ValueError(f"Invalid Mongo suite prefix: {path.name}")
        if prefix in owners:
            raise ValueError(f"Duplicate Mongo suite prefix in {owners[prefix]} and {path.name}")
        owners[prefix] = path.name
    if not owners:
        raise ValueError("Mongo suite registry must not be empty")
    return sorted(owners)


def suite_filter(directory=SUITES):
    return " | ".join(f"test({prefix})" for prefix in load_prefixes(directory))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    output = parser.add_mutually_exclusive_group(required=True)
    output.add_argument("--filter", action="store_true")
    output.add_argument("--prefixes", action="store_true")
    args = parser.parse_args()
    try:
        print(suite_filter() if args.filter else json.dumps(load_prefixes()))
    except (OSError, ValueError) as error:
        parser.exit(1, f"Mongo suite registry failed: {error}\n")


if __name__ == "__main__":
    main()
