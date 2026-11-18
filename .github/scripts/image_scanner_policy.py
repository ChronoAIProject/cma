"""Permit only the frozen image scanner policy; forbid repository scan bypasses."""
import hashlib
import json
import re
import subprocess
from pathlib import Path

BUNDLE = Path(".github/scripts/image-secret-scanner")
# The manifest binds every scanner file, including the detector policy.
# This cache candidate requires exact-bundle review before adoption.
MANIFEST_SHA256 = "4734b94a8f12b2116c35c4c5002d8d13e9fc4c6ed6364a805d5f8cfdbf761c16"
CLASSIFICATION_DIR = Path(".github/image-secret-classifications")
# Archive SHA256 -> (packet SHA256, SA-01 review SHA256). Add an entry only
# from an exact SA-01 verdict; a packet's self-declared reviewer is not approval.
APPROVED_CLASSIFICATIONS: dict[str, tuple[str, str]] = {}


class PolicyRefusal(ValueError):
    """Diagnostics must never echo an offending file's contents."""


def require(condition, message):
    if not condition:
        raise PolicyRefusal(message)


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def load_classifications(root, archive_digest):
    require(re.fullmatch(r"[a-f0-9]{64}", archive_digest) is not None, "Invalid classification archive identity")
    packet = root / CLASSIFICATION_DIR / (archive_digest + ".json")
    pins = APPROVED_CLASSIFICATIONS.get(archive_digest)
    if pins is None:
        require(not packet.exists() and not packet.is_symlink(), "Classification packet has no reviewed pin")
        return None
    require(len(pins) == 2 and all(isinstance(pin, str) and re.fullmatch(r"[a-f0-9]{64}", pin) for pin in pins),
            "Invalid classification review pin")
    require(not packet.is_symlink() and not packet.parent.is_symlink(), "Classification packet must be a regular file")
    data = packet.read_bytes()
    require(hashlib.sha256(data).hexdigest() == pins[0], "Classification packet differs from the pinned review")
    parsed = json.loads(data)
    require(parsed["archive_sha256"] == archive_digest
            and parsed["review"]["reviewer"] == "solution-architect-01"
            and parsed["review"]["report_sha256"] == pins[1], "Classification review identity differs")
    return data


def verify_bundle(root, manifest_sha256=MANIFEST_SHA256):
    manifest_path = BUNDLE / "SOURCE-MANIFEST.json"
    require(not (root / manifest_path).is_symlink() and sha256(root / manifest_path) == manifest_sha256,
            "Image scanner source manifest differs from the pinned review")
    manifest = json.loads((root / manifest_path).read_text())
    require(manifest["schema"] == "scan-image-secrets-source-v1", "Unknown image scanner source manifest")
    frozen = {manifest_path}
    for entry in manifest["files"]:
        relative = Path(entry["path"])
        require(not relative.is_absolute() and ".." not in relative.parts, "Unsafe scanner manifest path")
        name = BUNDLE / relative
        require(name not in frozen and not (root / name).is_symlink()
                and sha256(root / name) == entry["sha256"], "Image scanner source differs from the pinned review")
        frozen.add(name)
    require(BUNDLE / "gitleaks.toml" in frozen, "Scanner policy is absent from the frozen manifest")
    return frozen


def validate(root, paths, manifest_sha256=MANIFEST_SHA256):
    frozen = verify_bundle(root, manifest_sha256)
    require(frozen.issubset(set(paths)), "Frozen scanner files must be tracked with the publishing workflow")
    packets = {CLASSIFICATION_DIR / (digest + ".json") for digest in APPROVED_CLASSIFICATIONS}
    require(packets.issubset(set(paths)), "Reviewed classification packets must be tracked")
    for digest in APPROVED_CLASSIFICATIONS:
        load_classifications(root, digest)
    for name in paths:
        path = Path(name)
        require(not path.is_absolute() and ".." not in path.parts, "Unsafe repository path")
        if path == CLASSIFICATION_DIR or CLASSIFICATION_DIR in path.parents:
            require(path in packets, "Classification packet has no reviewed pin")
        if path == BUNDLE or BUNDLE in path.parents:
            require(path in frozen, "Unreviewed file in image scanner bundle")
        if path.name in (".gitleaks.toml", "gitleaks.toml", ".gitleaksignore"):
            require(path == BUNDLE / "gitleaks.toml", "Repository scanner overrides are forbidden")
        if path not in frozen and (root / path).is_file():
            require(b"gitleaks:" + b"allow" not in (root / path).read_bytes(),
                    "Repository scanner bypass annotations are forbidden")
    return frozen


def main():
    try:
        # CI checks a fresh checkout. Inspect tracked source, excluding local
        # dependency caches while preserving every file that can ship.
        listed = subprocess.run(["git", "ls-files", "-z"], check=True, capture_output=True)
        paths = [Path(name) for name in listed.stdout.decode().split("\0") if name]
        validate(Path.cwd(), paths)
    except (PolicyRefusal, OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError):
        print("::error::Repository secret policy or frozen image scanner integrity failed; details withheld")
        return 1
    print("Repository secret policy and frozen image scanner integrity passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
