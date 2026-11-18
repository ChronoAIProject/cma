#!/usr/bin/env python3
"""Resolve canonical public image names from the shared selector build manifest."""
import json
import re
import sys
from pathlib import Path

MANIFEST = Path(__file__).resolve().with_name("deployables.json")
ALIASES = {"am": "api", "eg": "cmaeg"}


def registry():
    data = json.loads(MANIFEST.read_text())
    if not isinstance(data, dict) or len(data) != 7 or not all(re.fullmatch(r"cma-[a-z]+(?:-[a-z]+)*", name) for name in data):
        raise ValueError("Invalid public deployables registry")
    return data


def name(component):
    selected = [key for key, value in registry().items() if value["component"] == ALIASES.get(component, component)]
    if len(selected) != 1:
        raise ValueError("Unknown deployable component")
    return selected[0]


def components():
    aliases = {value: key for key, value in ALIASES.items()}
    return {aliases.get(value["component"], value["component"]): key for key, value in registry().items()}


if __name__ == "__main__":
    try:
        if len(sys.argv) != 2:
            raise ValueError("Expected one component")
        print(name(sys.argv[1]))
    except (ValueError, TypeError, OSError):
        print("Invalid deployable", file=sys.stderr)
        raise SystemExit(1)
