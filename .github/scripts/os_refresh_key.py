#!/usr/bin/env python3
"""The OS_SECURITY_REFRESH build argument for one image component.

Runtime images (runtime and runtime-omarchy) install Debian packages from one
pinned snapshot.debian.org state, declared once in the runtime Dockerfile as
`ARG DEBIAN_SNAPSHOT=<YYYYMMDDTHHMMSSZ>`. Their package layers then depend on
that snapshot alone, so their refresh key is the snapshot: the layers come from
cache until someone deliberately bumps it, and builds stop depending on the
Debian archive being reachable. A missing or ambiguous pin is refused rather
than silently falling back to a per-build refresh.

Other images are not snapshot-pinned and keep a fresh per-build key, so they
still receive OS security updates on every build.

CLI: os_refresh_key.py <component> <runtime-dockerfile> <per-build-key>
prints `value=<key>` for $GITHUB_OUTPUT.
"""
import re
import sys
from pathlib import Path

RUNTIME_COMPONENTS = frozenset({"runtime", "runtime-omarchy"})
SNAPSHOT = re.compile(r"ARG DEBIAN_SNAPSHOT=([0-9]{8}T[0-9]{6}Z)")
DECLARED = re.compile(r"\s*ARG\s+DEBIAN_SNAPSHOT\s*=")


class SnapshotPinError(ValueError):
    pass


def pinned_snapshot(dockerfile_text):
    declared = [line for line in dockerfile_text.splitlines() if DECLARED.match(line)]
    valid = [match.group(1) for line in declared if (match := SNAPSHOT.fullmatch(line))]
    if len(declared) != 1 or len(valid) != 1:
        raise SnapshotPinError(
            "The runtime Dockerfile must declare exactly one `ARG DEBIAN_SNAPSHOT=YYYYMMDDTHHMMSSZ` "
            f"line; found {len(declared)} declaration(s), {len(valid)} valid. Restore the single "
            "pinned snapshot declaration before building runtime images.")
    return valid[0]


def refresh_key(component, runtime_dockerfile_text, per_build_key):
    if component in RUNTIME_COMPONENTS:
        return pinned_snapshot(runtime_dockerfile_text)
    if not per_build_key or "\n" in per_build_key:
        raise SnapshotPinError("A non-runtime image needs a single-line per-build refresh key")
    return per_build_key


def main(argv):
    if len(argv) != 4:
        print(__doc__.strip().splitlines()[-2], file=sys.stderr)
        return 2
    component, dockerfile, per_build_key = argv[1:]
    try:
        print("value=" + refresh_key(component, Path(dockerfile).read_text(), per_build_key))
    except (OSError, SnapshotPinError) as error:
        print(f"::error title=OS refresh key unavailable::{error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
