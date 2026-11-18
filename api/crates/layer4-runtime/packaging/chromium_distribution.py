"""Remove only the verified distribution integration file before final copying."""
import hashlib
import json
import subprocess
from pathlib import Path

from prune import anchored


def strip(root, expected, installed):
    root = root.resolve(strict=True)
    if installed.splitlines() != [expected["package"][field]
                                  for field in ("name", "version", "architecture")]:
        raise ValueError("Chromium package identity differs")
    target = anchored(root, expected["file"]["path"])
    if (target.is_symlink() or not target.is_file()
            or target.stat().st_size != expected["file"]["bytes"]
            or hashlib.sha256(target.read_bytes()).hexdigest() != expected["file"]["sha256"]):
        # Unexpected bytes need investigation, not silent deletion or allowance.
        raise ValueError("Chromium distribution file differs")
    target.unlink()


def main():
    try:
        expected = json.loads(Path(__file__).with_name("chromium-distribution.json").read_text())
        installed = subprocess.run(
            ["dpkg-query", "-W", "-f=${Package}\n${Version}\n${Architecture}\n", "chromium"],
            capture_output=True, check=True, text=True, timeout=30,
        )
        strip(Path("/"), expected, installed.stdout)
    except (ValueError, OSError, KeyError, TypeError, subprocess.SubprocessError):
        print("chromium-distribution-strip=FAIL")
        return 1
    print("chromium-distribution-strip=PASS")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
