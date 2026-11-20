"""Permit only the frozen image scanner policy; forbid repository scan bypasses."""
import hashlib
import json
import re
import subprocess
from pathlib import Path

BUNDLE = Path(".github/scripts/image-secret-scanner")
# Exact candidate source binding, including detector policy and cache tests.
# Adoption of changed bytes requires independent review before merge.
MANIFEST_SHA256 = "02cc16f799e7a549a60a4c7b2431421b64dbc59bfc8fa4e8d7744c15cdb6e0d6"
CLASSIFICATION_DIR = Path(".github/image-secret-classifications")
# Archive SHA256 -> (packet SHA256, SA-01 review SHA256). Add an entry only
# from an exact SA-01 verdict; a packet's self-declared reviewer is not approval.
APPROVED_CLASSIFICATIONS: dict[str, tuple[str, str]] = {}
# (content manifest SHA256, authentic SA-01 review SHA256). Remains disabled
# until the exact catalog and resolver have received that independent verdict.
APPROVED_CONTENT_POLICY = (
    "3cfb89bf19c4ddda0fad950c284078852a88a13e3ec91b44183504b3f306d79e",
    "9eae89fca11ab926dbf837cd1813dd1468cf83fed418fecf80f629e50d66f8bb",
)
PACKAGE_EVIDENCE = Path(".github/scripts/image-package-evidence.json")
OMARCHY_PACKAGE_EVIDENCE = Path(".github/scripts/image-omarchy-package-evidence.json")
# Independently reviewed evidence and verdict digests. Evidence gathered from
# the candidate image cannot supply either pin.
APPROVED_PACKAGE_EVIDENCE = (
    "44656e1ed489fa99ea563879f7b3c0b43c3408c8e460bf3d64f87da7e3376b0d",
    "e1e65f4dee3c3d3fc15df3c70c5a29c8f029cd80aa52cd335ab5d41f4087c23a",
)
APPROVED_OMARCHY_PACKAGE_EVIDENCE = (
    "525f80566d97b91f1a963e72af828a5423d5af8827892f27b576e17e005b1d04",
    "9293998f0889f40a3c7f89a9e0b26b085033012fed598a4f491cb0cb6a298e3b",
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
