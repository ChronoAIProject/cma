#!/usr/bin/env python3
"""Build a reproducible, stdlib-only Python zipapp outside the source tree."""
import argparse
import hashlib
from pathlib import Path
import zipfile


def package(output):
    source = Path(__file__).resolve().parent
    files = {name: (source / name).read_bytes() for name in
             ("rehearsal_events.py", "watch_hook_events.py", "hook_events.py")}
    files["__main__.py"] = b"from rehearsal_events import main\nraise SystemExit(main())\n"
    output.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        for name, contents in sorted(files.items()):
            info = zipfile.ZipInfo(name, date_time=(2026, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = 0o100644 << 16
            archive.writestr(info, contents)
    return hashlib.sha256(output.read_bytes()).hexdigest()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    print(package(args.output), args.output)
