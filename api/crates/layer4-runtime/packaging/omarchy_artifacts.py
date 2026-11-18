"""Remove reviewed examples while retaining executable modules and package state."""
import json
from pathlib import Path

from artifact_edits import sha, transform
from prune import anchored
from wheel_strips import rebuild


def read(root, name):
    from pathlib import PurePosixPath
    path = PurePosixPath(name)
    if path.is_absolute() or ".." in path.parts or path.as_posix() != name:
        raise ValueError("Invalid artifact path")
    source = anchored(root, name)
    if source.is_symlink() or not source.is_file() or source.stat().st_size > 32 * 1024 * 1024:
        raise ValueError("Invalid artifact source")
    return source.read_bytes()


def prepare(root, policy):
    root = root.resolve(strict=True)
    if policy.get("schema") != "omarchy-artifact-strips-v1":
        raise ValueError("Invalid artifact strip policy")
    names = [item["path"] for kind in ("remove", "edit", "wheels") for item in policy[kind]]
    if len(set(names)) != len(names):
        raise ValueError("Duplicate artifact strip path")
    changes, removals = [], []
    for entry in policy["remove"]:
        data = read(root, entry["path"])
        if sha(data) != entry["source_sha256"] or len(data) != entry["bytes"]:
            raise ValueError("Disposable artifact differs")
        removals.append(anchored(root, entry["path"]))
    for entry in policy["edit"]:
        changes.append((anchored(root, entry["path"]), transform(read(root, entry["path"]), entry)))
    for entry in policy["wheels"]:
        path = anchored(root, entry["path"])
        if path.is_symlink() or not path.is_file() or path.stat().st_size > 32 * 1024 * 1024:
            raise ValueError("Invalid wheel path")
        changes.append((path, rebuild(path.read_bytes(), entry)))
    # Validate the entire package before mutating any file. No image code executes.
    for path, data in changes:
        path.write_bytes(data)
    for path in removals:
        path.unlink()


def main():
    try:
        policy = json.loads(Path(__file__).with_name("omarchy-artifact-strips.json").read_text())
        prepare(Path("/"), policy)
    except (OSError, ValueError, KeyError, TypeError):
        print("omarchy-artifact-strip=FAIL")
        return 1
    print("omarchy-artifact-strip=PASS")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
