"""Keep the packaged binutils notices and update their obsolete CVS source URL."""

import hashlib
from pathlib import Path
import sys


PACKAGES = ("binutils", "binutils-common", "libctf-nobfd0", "libsframe1")
NOTICE_SHA256 = "20a9818b14f941467bcc0f459a1b1951e3bfecc1cc259e5004f8b9a80d7dc804"
LEGACY_LINE_SHA256 = "a03090a67c9eca367f9d8cfea1babcb7870889ed708483612edb44a6d7b68d7b"
SOURCE_LOCATION = b"  https://sourceware.org/pub/binutils/releases/\n"


def update_notice(path):
    original = path.read_bytes()
    if b"cvs://" not in original:
        return False
    # Limit the edit to the reviewed optional locator. A changed vendor notice
    # with a CVS reference needs review before any of its bytes can be replaced.
    if hashlib.sha256(original).hexdigest() != NOTICE_SHA256:
        raise ValueError("Unrecognized binutils notice with a CVS locator")
    lines = original.splitlines(keepends=True)
    matches = [i for i, line in enumerate(lines) if hashlib.sha256(line).hexdigest() == LEGACY_LINE_SHA256]
    if len(matches) != 1:
        raise ValueError("Expected one reviewed binutils source locator")
    lines[matches[0]] = SOURCE_LOCATION
    path.write_bytes(b"".join(lines))
    return True


def normalize(root):
    documents = Path(root).resolve() / "usr/share/doc"
    notices = {documents / package / "copyright" for package in PACKAGES}
    changed = 0
    for notice in sorted(notices):
        try:
            target = notice.resolve(strict=True)
        except FileNotFoundError:
            # Package selection varies by architecture. A dangling notice is
            # different from a package which did not install this path.
            if notice.is_symlink():
                raise
            continue
        if target not in notices or not target.is_file():
            raise ValueError("Unexpected binutils notice target")
        changed += update_notice(target)
    return changed


if __name__ == "__main__":
    print(f"Updated binutils source locations: {normalize(sys.argv[1] if len(sys.argv) > 1 else '/')}")
