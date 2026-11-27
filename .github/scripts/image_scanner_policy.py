"""Permit only the frozen image scanner policy; forbid repository scan bypasses."""
import hashlib
import json
import re
import subprocess
from pathlib import Path

BUNDLE = Path(".github/scripts/image-secret-scanner")
# Exact candidate source binding, including detector policy and cache tests.
# Adoption of changed bytes requires independent review before merge.
MANIFEST_SHA256 = "4ad252394ac8d144da7b316f138c71f90ebf9578bce9f3d118b77bee7b8da11c"
CLASSIFICATION_DIR = Path(".github/image-secret-classifications")
# Archive SHA256 -> (packet SHA256, SA-01 review SHA256). Add an entry only
# from an exact SA-01 verdict; a packet's self-declared reviewer is not approval.
APPROVED_CLASSIFICATIONS: dict[str, tuple[str, str]] = {}
# (content manifest SHA256, authentic SA-01 review SHA256). Remains disabled
# until the exact catalog and resolver have received that independent verdict.
APPROVED_CONTENT_POLICY = (
    "aafd541c0243e1931a0d2387e4de12d71e1c265f6986c78362f3c246b430233a",
    "6e6a175226dc22d5f681780e10a4b2b5bbc9aac163859d61a4af1fcf9bb6d818",
)
PACKAGE_EVIDENCE = Path(".github/scripts/image-package-evidence.json")
OMARCHY_PACKAGE_EVIDENCE = Path(".github/scripts/image-omarchy-package-evidence.json")
# Independently reviewed evidence and verdict digests. Evidence gathered from
# the candidate image cannot supply either pin.
APPROVED_PACKAGE_EVIDENCE = (
    "34ce0c58b09484311a636bcd0b3d8a0d125cb6a990cf2eeaa1e89c82fddffbf0",
    "df0fabb0c06b3ebe8f144c952c6d35bc8bf201209c55ce9b46e9fe2dadac5a66",
)
APPROVED_OMARCHY_PACKAGE_EVIDENCE = (
    "9b9b5ef741a18fdf494d7f7fd3c31b4bb9139814410cf378aae9df1aa60513f7",
    "df0fabb0c06b3ebe8f144c952c6d35bc8bf201209c55ce9b46e9fe2dadac5a66",
)
# Runtime scope requires a new exact-manifest review before activation.
CONTENT_FILES = frozenset({"image_content_policy.py", "image_content_identity.py", "image-content-policy.json",
                           "image_runtime_headers.py",
                           "image_bundle_contexts.py", "image_bundle_syntax.cjs",
                           "image-content-vendor/babel-parser-7.29.3.cjs", "image-content-vendor/BABEL-LICENSE",
                           "image-content-vendor/PROVENANCE.json"})


def load_content_policy(root):
    if APPROVED_CONTENT_POLICY is None:
        return None
    manifest_sha, review_sha = APPROVED_CONTENT_POLICY
    require(all(isinstance(value, str) and re.fullmatch(r"[a-f0-9]{64}", value)
                for value in (manifest_sha, review_sha)), "Invalid content review pin")
    directory = root / ".github/scripts"
    manifest = directory / "image-content-manifest.json"
    require(not manifest.is_symlink() and sha256(manifest) == manifest_sha,
            "Content policy manifest differs from the pinned review")
    files = json.loads(manifest.read_bytes())
    require(files.get("schema") == "cma-image-content-source-v1" and set(files["files"]) == CONTENT_FILES,
            "Content policy source inventory differs")
    for name, digest in files["files"].items():
        path = directory / name
        require(not path.is_symlink() and path.is_file() and sha256(path) == digest,
                "Content policy source differs from the pinned review")
    return directory / "image-content-policy.json", review_sha


class PolicyRefusal(ValueError):
    """Diagnostics must never echo an offending file's contents."""


def require(condition, message):
    if not condition:
        raise PolicyRefusal(message)


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def package_evidence_pins():
    return {
        "runtime": (PACKAGE_EVIDENCE, APPROVED_PACKAGE_EVIDENCE),
        "runtime-omarchy": (OMARCHY_PACKAGE_EVIDENCE, APPROVED_OMARCHY_PACKAGE_EVIDENCE),
    }


def load_package_evidence(root, component="runtime"):
    relative, pins = package_evidence_pins()[component]
    path = root / relative
    if pins is None:
        require(not path.exists() and not path.is_symlink(), "Package evidence has no reviewed pin")
        return None
    require(len(pins) == 2 and all(isinstance(pin, str) and re.fullmatch(r"[a-f0-9]{64}", pin)
                                  for pin in pins), "Invalid package evidence pin")
    require(not path.is_symlink() and path.is_file(), "Package evidence must be a regular file")
    with path.open("rb") as stream:
        raw = stream.read(16 * 1024 * 1024 + 1)
    require(len(raw) <= 16 * 1024 * 1024 and hashlib.sha256(raw).hexdigest() == pins[0],
            "Package evidence differs from the pinned review")
    return raw, *pins


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
    load_content_policy(root)
    for component, (relative, _) in package_evidence_pins().items():
        evidence = load_package_evidence(root, component)
        require(evidence is None or relative in paths, "Reviewed package evidence must be tracked")
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
