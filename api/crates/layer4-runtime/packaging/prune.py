"""Remove development-only payloads before copying a runtime into clean layers."""
import argparse
import json
import os
import shutil
from pathlib import Path, PurePosixPath


def load_policy(path):
    value = json.loads(path.read_text())
    if (not isinstance(value, dict) or set(value) != {
            "schema", "remove", "desktop_remove", "library_roots", "development_directories"}
            or value["schema"] != "runtime-packaging-prune-v1"):
        raise ValueError("Invalid packaging policy")
    for key in ("remove", "desktop_remove", "library_roots", "development_directories"):
        items = value[key]
        if not isinstance(items, list) or len(items) > 50 or len(set(items)) != len(items):
            raise ValueError("Invalid packaging paths")
        for item in items:
            if (not isinstance(item, str) or not item or PurePosixPath(item).as_posix() != item
                    or PurePosixPath(item).is_absolute() or ".." in PurePosixPath(item).parts):
                raise ValueError("Invalid packaging path")
            if key == "development_directories" and "/" in item:
                raise ValueError("Invalid development directory")
    return value


def anchored(root, relative):
    candidate = root / relative
    for ancestor in candidate.parents:
        if ancestor == root:
            return candidate
        if ancestor.is_symlink():
            raise ValueError("Packaging path crosses a symlink")
    raise ValueError("Packaging path escapes its root")


def prune(root, policy, *, desktop=False):
    root = root.resolve(strict=True)
    if not root.is_dir():
        raise ValueError("Packaging root is not a directory")
    # Validate the whole deletion set before mutating the prepared filesystem.
    selected = policy["remove"] + (policy["desktop_remove"] if desktop else [])
    removals = [anchored(root, name) for name in selected]
    desktop_targets = {anchored(root, name) for name in policy["desktop_remove"]} if desktop else set()
    visited = 0
    for name in policy["library_roots"]:
        library = anchored(root, name)
        if library.is_symlink():
            raise ValueError("Packaging library root is a symlink")
        for directory, directories, _files in os.walk(library, followlinks=False):
            visited += 1
            if visited > 100000:
                raise ValueError("Packaging traversal exceeds its bound")
            for child in list(directories):
                path = Path(directory) / child
                if child in policy["development_directories"]:
                    removals.append(path)
                    directories.remove(child)
    removed = 0
    for path in sorted(set(removals), key=lambda item: len(item.parts), reverse=True):
        if path.is_symlink() or path.is_file():
            path.unlink()
            removed += 1
        elif path.is_dir():
            if path in desktop_targets:
                shutil.rmtree(path)
                removed += 1
                continue
            # Preserve directory ownership/modes (notably sticky /tmp), while
            # removing its build-time payload from the eventual copied tree.
            for entry in path.iterdir():
                if entry.is_dir() and not entry.is_symlink():
                    shutil.rmtree(entry)
                else:
                    entry.unlink()
            removed += 1
    return removed


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--desktop", choices=("0", "1"), default="0")
    args = parser.parse_args()
    try:
        count = prune(args.root, load_policy(Path(__file__).with_name("prune-policy.json")),
                      desktop=args.desktop == "1")
    except (ValueError, OSError, TypeError):
        print("runtime-packaging=FAIL")
        return 1
    print("runtime-packaging=PASS removed=" + str(count))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
