"""Leave machine identity initialization to each container's startup environment."""

from pathlib import Path
import sys


def normalize(root):
    root = Path(root)
    identity = root / "etc/machine-id"
    identity.parent.mkdir(parents=True, exist_ok=True)
    # Replace links instead of following a base image's target outside this root.
    identity.unlink(missing_ok=True)
    identity.touch(mode=0o444)
    bus_identity = root / "var/lib/dbus/machine-id"
    bus_identity.parent.mkdir(parents=True, exist_ok=True)
    bus_identity.unlink(missing_ok=True)
    bus_identity.symlink_to("/etc/machine-id")


if __name__ == "__main__":
    normalize(sys.argv[1] if len(sys.argv) > 1 else "/")
