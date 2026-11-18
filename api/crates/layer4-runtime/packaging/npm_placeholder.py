"""Remove one reviewed documentation paragraph without changing npm's options."""

import argparse
import hashlib
import json
from pathlib import Path

from prune import anchored

ROOT = "usr/local/lib/node_modules/npm/"
FILE = ROOT + "node_modules/@npmcli/config/lib/definitions/definitions.js"
SOURCE = "cf8f31b79a4428b25e2dac5b080c3b9e0fc03dd8dea54124a0c6cf05e1526630"
RESULT = "59647114bb481d3fb699d6c4d7bc3aa875e3cd05a4d7a284eb46552f7dc02be2"
PARAGRAPH = "b0c7465d0b47b6a63c2c25ff1e9036cb35e70f5921e07bedc0adac10f023914d"


def read(root, relative):
    path = anchored(root, relative)
    if path.is_symlink() or not path.is_file() or path.stat().st_size > 1024 * 1024:
        raise ValueError("Invalid npm source")
    return path.read_bytes()


def stripped(data):
    # These offsets cover only the example paragraph of key.description in the
    # exact upstream module. No option, type, default, or flatten code is edited.
    if hashlib.sha256(data).hexdigest() != SOURCE:
        raise ValueError("Unreviewed npm source")
    if hashlib.sha256(data[44957:45082]).hexdigest() != PARAGRAPH:
        raise ValueError("Unreviewed npm paragraph")
    result = data[:44957] + data[45082:]
    if hashlib.sha256(result).hexdigest() != RESULT:
        raise ValueError("Invalid npm result")
    return result


def prepare(root):
    root = root.resolve(strict=True)
    for relative, name, version in (
        (ROOT + "package.json", "npm", "12.0.2"),
        (ROOT + "node_modules/@npmcli/config/package.json", "@npmcli/config", "11.0.1"),
    ):
        metadata = json.loads(read(root, relative))
        if metadata.get("name") != name or metadata.get("version") != version:
            raise ValueError("Unreviewed npm package")
    result = stripped(read(root, FILE))
    anchored(root, FILE).write_bytes(result)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, required=True)
    args = parser.parse_args()
    try:
        prepare(args.root)
    except (OSError, ValueError, TypeError):
        print("npm-documentation-strip=FAIL")
        return 1
    print("npm-documentation-strip=PASS")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
