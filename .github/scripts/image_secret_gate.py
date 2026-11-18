"""Bind publication to the reviewed scanner, explicit mode and exact archive."""
import hashlib
import json
import os
import re
import subprocess
import tempfile
from pathlib import Path

from image_scanner_policy import PolicyRefusal, load_classifications, verify_bundle

SCANNER = Path(__file__).resolve().parent / "image-secret-scanner" / "scan-image-secrets.sh"
EMPTY_POLICY = '{"version":1,"entries":[]}\n'
PUBLIC_CLASSES = frozenset({"library-example", "test-fixture", "public-material"})


class ScanRefusal(ValueError):
    """Fixed diagnostics that never include scanner output or image content."""


def checksum(path):
    result = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            result.update(block)
    return result.hexdigest()


def run_scan(*args):
    # Archive scans need no GitHub, registry, or deployment credential.
    env = {key: value for key, value in os.environ.items()
           if key in ("PATH", "HOME", "LANG", "LC_ALL", "TMPDIR", "RUNNER_TEMP")}
    try:
        result = subprocess.run(args, capture_output=True, timeout=1200, env=env, check=False)
    except (OSError, subprocess.TimeoutExpired) as error:
        raise ScanRefusal("Image secret scanner unavailable or timed out; publication stopped") from error
    if result.returncode != 0:
        raise ScanRefusal("Image secret scan has blocking findings or did not complete; publication stopped")


def require(condition, message="Image secret scan receipt is incomplete, unbound, or has blockers; publication stopped"):
    if not condition:
        raise ScanRefusal(message)


def validate_receipt(report, archive_digest, mode, packet):
    policy = report["policy"]
    require(report.get("schema") == "image-secret-scan-v1" and report.get("scanner_version") == "1.0.3"
            and report.get("complete") is True and report.get("status") == "CLEAN"
            and type(report.get("exit_code")) is int and report["exit_code"] == 0 and not report.get("error")
            and report.get("blocking_rules") == mode and report.get("blocking_findings") == []
            and report.get("allowlisted") == [] and report.get("gitleaks_version") == "8.30.1"
            and policy.get("allowlist_sha256") == hashlib.sha256(EMPTY_POLICY.encode()).hexdigest()
            and policy.get("gitleaks_rules_sha256") == checksum(SCANNER.parent / "gitleaks.toml")
            and report.get("archive_sha256") == archive_digest)
    decisions = {}
    if packet is None:
        require("classifications_sha256" not in policy and "classification_review_sha256" not in policy)
    else:
        parsed = json.loads(packet)
        require(mode == "credential" and policy.get("classifications_sha256") == hashlib.sha256(packet).hexdigest()
                and policy.get("classification_review_sha256") == parsed["review"]["report_sha256"])
        decisions = {entry["finding_id"]: entry["classification"] for entry in parsed["entries"]}
        require(len(decisions) == len(parsed["entries"]))
    findings = report["findings"]
    require(isinstance(findings, list))
    observed, advisory, reviewed = set(), [], []
    for finding in findings:
        identity = finding["finding_id"]
        require(isinstance(identity, str) and re.fullmatch(r"[a-f0-9]{64}", identity)
                and identity not in observed)
        observed.add(identity)
        classification = decisions.get(identity)
        if classification is not None:
            require(classification in PUBLIC_CLASSES)
            reviewed.append(dict(finding, classification=classification))
        else:
            require(mode == "credential" and finding["rule"] == "gitleaks.image-high-entropy")
            advisory.append(finding)
    require(set(decisions).issubset(observed) and report.get("advisory_findings") == advisory
            and report.get("reviewed_classifications") == reviewed)
    counts = {"detected": len(findings), "blocking": 0, "advisory": len(advisory),
              "reviewed": len(reviewed), "allowlisted": 0}
    require(report.get("counts") == counts and all(type(value) is int for value in report["counts"].values()))


def scan_archive(archive, output, run_command=run_scan, *, blocking_rules="all", detector_cache=None,
                 detector_cache_output=None):
    require(blocking_rules in ("all", "credential"), "Unapproved image scan mode; publication stopped")
    require(not output.exists(), "Image secret scan output must be new; publication stopped")
    original = checksum(archive)
    try:
        root = Path(__file__).resolve().parents[2]
        verify_bundle(root)
        packet = load_classifications(root, original) if blocking_rules == "credential" else None
    except (PolicyRefusal, OSError, ValueError, KeyError, TypeError) as error:
        raise ScanRefusal("Image scanner or classification packet differs from the pinned review; publication stopped") from error
    output.parent.mkdir(parents=True, exist_ok=True)
    # Reviewed classifications are artifact-bound. The generic scanner allowlist
    # stays empty so it cannot hide candidates before the Policy A decision.
    with tempfile.TemporaryDirectory(prefix="image-scan-policy-", dir=output.parent) as policy_dir:
        policy = Path(policy_dir) / "no-exceptions.json"
        policy.write_text(EMPTY_POLICY, encoding="utf-8")
        options = ["--blocking-rules", blocking_rules]
        if detector_cache is not None:
            options += ["--detector-cache", str(detector_cache)]
        if detector_cache_output is not None:
            options += ["--detector-cache-output", str(detector_cache_output)]
        if packet is not None:
            classifications = Path(policy_dir) / "classifications.json"
            classifications.write_bytes(packet)
            options += ["--classifications", str(classifications)]
        run_command("bash", str(SCANNER), str(archive), "--input-kind", "archive",
                    "--output", str(output), "--allowlist", str(policy),
                    "--expected-archive-sha256", original, *options)
    try:
        report = json.loads((output / "REPORT.json").read_text())
        validate_receipt(report, original, blocking_rules, packet)
        require(checksum(archive) == original)
    except (OSError, ValueError, AttributeError, KeyError, TypeError) as error:
        raise ScanRefusal("Image secret scan receipt is missing or invalid; publication stopped") from error
    counts = report["counts"]
    print(f"Image secret scan passed: mode={blocking_rules}, blockers=0, "
          f"advisory={counts['advisory']}, reviewed={counts['reviewed']}, archive=sha256:{original}")
    return original


def scan_staging_archive(archive, output, run_command=run_scan, **cache_options):
    """The reviewed staging-track contract explicitly selects Policy A."""
    return scan_archive(archive, output, run_command, blocking_rules="credential", **cache_options)
