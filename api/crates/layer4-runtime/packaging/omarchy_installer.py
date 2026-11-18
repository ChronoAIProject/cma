"""Strip an optional credential-bearing installer and its exact callers."""
import argparse
import json
from pathlib import Path

from documentation import read, sha, stripped
from prune import anchored


def prepare(root, policy):
    root = root.resolve(strict=True)
    if policy.get("schema") != "omarchy-optional-installer-strips-v1":
        raise ValueError("Invalid installer strip policy")
    removals, edits = policy["remove"], policy["edit"]
    if len(removals) != 1 or len(edits) != 2:
        raise ValueError("Invalid installer strip inventory")
    names = [entry["path"] for entry in removals + edits]
    if len(set(names)) != len(names) or any(not name.startswith("usr/share/omarchy/")
                                          for name in names):
        raise ValueError("Invalid installer strip paths")
    changed = []
    for entry in removals:
        data = read(root, entry["path"])
        if len(data) != entry["bytes"] or sha(data) != entry["source_sha256"]:
            raise ValueError("Installer source differs")
    for entry in edits:
        changed.append((anchored(root, entry["path"]), stripped(read(root, entry["path"]), entry)))
    # All three originals must match before deleting the installer or editing a caller.
    for path, result in changed:
        path.write_bytes(result)
    for entry in removals:
        anchored(root, entry["path"]).unlink()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", required=True, type=Path)
    args = parser.parse_args()
    try:
        policy = json.loads(Path(__file__).with_name("omarchy-installer-strips.json").read_text())
        prepare(args.root, policy)
    except (OSError, ValueError, TypeError, KeyError):
        print("omarchy-optional-installer-strip=FAIL")
        return 1
    print("omarchy-optional-installer-strip=PASS")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
