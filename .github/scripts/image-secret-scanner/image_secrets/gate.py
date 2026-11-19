"""Publication decisions are separate from complete image detection."""

import re

from .common import ScanError, digest_bytes, json_bytes

CLASSIFICATIONS = frozenset({"library-example", "test-fixture", "public-material", "REAL"})
ADVISORY_RULES = frozenset({"gitleaks.image-high-entropy"})
PRIVATE_KEY_RULES = frozenset({"content.private-key-header", "content.binary-private-key",
                              "content.openpgp-private-key", "gitleaks.private-key", "path.ssh-key"})
SELF_TEST_RULES = frozenset({"content.private-key-header", "content.binary-private-key", "gitleaks.private-key"})
SELF_TEST_ADVISORY = "published-library-self-test-vector"
EXECUTABLE_TEST_ADVISORY = "published-executable-self-test-vector"
TEST_VECTOR_ADVISORIES = frozenset({SELF_TEST_ADVISORY, EXECUTABLE_TEST_ADVISORY})
DIGEST_FRAME_RULE = "content.binary-private-key"
DIGEST_FRAME_ADVISORY = "published-digest-encoding"
DISTRIBUTION_RULE = "gitleaks.gcp-api-key"
DISTRIBUTION_ADVISORY = "published-distribution-configuration"
CERTIFICATE_RULE = "path.key-or-certificate"
CERTIFICATE_ADVISORY = "published-package-certificate-bundle"
ARCH_CERTIFICATE_ADVISORY = "published-arch-trust-certificate"
PACKAGE_ADVISORY_RULES = {
    SELF_TEST_ADVISORY: SELF_TEST_RULES,
    EXECUTABLE_TEST_ADVISORY: SELF_TEST_RULES,
    DIGEST_FRAME_ADVISORY: frozenset({DIGEST_FRAME_RULE}),
    DISTRIBUTION_ADVISORY: frozenset({DISTRIBUTION_RULE}),
    CERTIFICATE_ADVISORY: frozenset({CERTIFICATE_RULE}),
    ARCH_CERTIFICATE_ADVISORY: frozenset({CERTIFICATE_RULE}),
}


def package_advisory_allowed(classification, rule, component, member):
    if rule not in PACKAGE_ADVISORY_RULES.get(classification, ()):
        return False
    if classification == CERTIFICATE_ADVISORY:
        return component == "runtime-omarchy" and member is not None
    if classification == ARCH_CERTIFICATE_ADVISORY:
        return component == "runtime-omarchy" and member is None
    return component in ("runtime", "runtime-omarchy") and member is None


def valid_digest(value):
    return isinstance(value, str) and re.fullmatch(r"[a-f0-9]{64}", value) is not None


def read_classifications(path, result):
    with path.open("rb") as stream:
        packet_bytes = stream.read(16 * 1024 * 1024 + 1)
    if len(packet_bytes) > 16 * 1024 * 1024:
        raise ScanError("classification-packet-too-large")
    packet = json_bytes(packet_bytes)
    fields = {
        "schema",
        "archive_sha256",
        "scanner_version",
        "gitleaks_rules_sha256",
        "allowlist_sha256",
        "review",
        "entries",
    }
    if not isinstance(packet, dict) or set(packet) != fields:
        raise ScanError("invalid-classification-packet")
    if (
        packet["schema"] != "image-secret-classifications-v1"
        or packet["scanner_version"] != result["scanner_version"]
        or packet["archive_sha256"] != result["archive_sha256"]
        or packet["gitleaks_rules_sha256"] != result["policy"]["gitleaks_rules_sha256"]
        or packet["allowlist_sha256"] != result["policy"]["allowlist_sha256"]
    ):
        raise ScanError("classification-artifact-or-policy-mismatch")
    review = packet["review"]
    if (
        not isinstance(review, dict)
        or set(review) != {"reviewer", "reference", "report_sha256"}
        or review["reviewer"] != "solution-architect-01"
        or not isinstance(review["reference"], str)
        or not review["reference"].strip()
        or not valid_digest(review["report_sha256"])
        or not isinstance(packet["entries"], list)
    ):
        raise ScanError("invalid-classification-review")
    entries = {}
    for entry in packet["entries"]:
        if (
            not isinstance(entry, dict)
            or set(entry) != {"finding_id", "classification", "reason"}
            or not valid_digest(entry["finding_id"])
            or not isinstance(entry["classification"], str)
            or entry["classification"] not in CLASSIFICATIONS
            or not isinstance(entry["reason"], str)
            or not entry["reason"].strip()
            or entry["finding_id"] in entries
        ):
            raise ScanError("invalid-classification-entry")
        entries[entry["finding_id"]] = entry["classification"]
    observed = {item["finding_id"] for item in result["findings"]}
    if not set(entries).issubset(observed):
        raise ScanError("classification-finding-not-observed")
    result["policy"]["classifications_sha256"] = digest_bytes(packet_bytes)
    result["policy"]["classification_review_sha256"] = review["report_sha256"]
    return entries


def assess(result, mode, classifications=None, *, verified_package_decisions=None):
    """Never suppress detection. Only an explicit mode changes blocking semantics."""
    if mode not in ("all", "credential"):
        raise ScanError("invalid-blocking-mode")
    if (classifications is not None or verified_package_decisions is not None) and mode != "credential":
        raise ScanError("classifications-require-credential-mode")
    if classifications is not None and verified_package_decisions is not None:
        raise ScanError("classification-sources-conflict")
    decisions = (read_classifications(classifications, result) if classifications
                 else verified_package_decisions or {})
    blocking, advisory, reviewed = [], [], []
    for finding in result["findings"]:
        classification = decisions.get(finding["finding_id"])
        if (verified_package_decisions is not None and finding.get("scope") == "layer"
                and package_advisory_allowed(classification, finding["rule"],
                                             result["policy"].get("staging_runtime_component"),
                                             finding.get("member"))):
            advisory.append(dict(finding, classification=classification))
        elif classification in PACKAGE_ADVISORY_RULES:
            blocking.append(finding)
        elif classification:
            item = dict(finding, classification=classification)
            reviewed.append(item)
            if classification == "REAL" or finding["rule"] in PRIVATE_KEY_RULES:
                blocking.append(item)
        elif (mode == "credential" and finding["rule"] == "path.key-or-certificate"
              and result.get("policy", {}).get("staging_runtime_component") in ("runtime", "runtime-omarchy")
              and valid_digest(finding.get("public_certificate_sha256"))):
            advisory.append(dict(finding, classification="public-material"))
        elif mode == "credential" and finding["rule"] in ADVISORY_RULES:
            advisory.append(finding)
        else:
            # Unknown/provider rules and unresolved path heuristics remain blocking.
            # A filename or extension alone cannot establish public certificate content.
            blocking.append(finding)
    result.update(
        blocking_rules=mode,
        blocking_findings=blocking,
        advisory_findings=advisory,
        reviewed_classifications=reviewed,
        counts={
            "detected": len(result["findings"]),
            "blocking": len(blocking),
            "advisory": len(advisory),
            "reviewed": len(reviewed),
            "allowlisted": len(result["allowlisted"]),
        },
    )
    result["status"] = "FINDINGS" if blocking else "CLEAN"
