"""Project a bounded failure receipt without values, names, or tool messages."""
import hashlib
import json
import re

OUTCOMES = frozenset({"scan_refused", "scanner_findings", "scanner_error", "scanner_timeout",
                      "scanner_unavailable", "receipt_invalid", "content_refused"})
DIGEST = re.compile(r"[a-f0-9]{64}")
# Every rule the pinned scanner can report. image_scan_evidence_tests checks this
# set against the scanner bundle, so a new rule cannot surface as unrecognized_rule.
RULES = frozenset({"path.key-or-certificate", "gitleaks.image-secret-assignment",
                  "gitleaks.generic-api-key", "gitleaks.image-high-entropy",
                  "gitleaks.image-hex-secret-assignment",
                  "content.private-key-header", "content.binary-private-key", "content.openpgp-private-key",
                  "content.service-account", "content.credential-url",
                  "config.secret-assignment", "path.auth-config", "path.env-file", "path.ssh-key",
                  "path.credential-file", "path.git-metadata", "path.docker-auth", "path.kubeconfig",
                  "path.aws-credentials", "path.azure-credentials", "path.gcp-credentials",
                  "path.cargo-credentials"})


# The pinned scanner owns these generic diagnostics. Other fixed scanner error
# identifiers receive an opaque fingerprint for correlation by its reviewers.
DIAGNOSTIC_CLASSES = frozenset({"scan_operation_failed", "classification_failed",
                                "scanner_reported_error", "diagnostic_unavailable"})
ERROR_IDENTIFIER = re.compile(r"[a-z][a-z0-9-]{0,127}")


def incomplete_diagnostic(report):
    if report.get("schema") == "image-scan-failure-v1":
        diagnostic = report.get("diagnostic")
        if isinstance(diagnostic, dict):
            kind = diagnostic.get("kind")
            fingerprint = diagnostic.get("sha256")
            if isinstance(kind, str) and kind in DIAGNOSTIC_CLASSES:
                result = {"kind": kind}
                if isinstance(fingerprint, str) and DIGEST.fullmatch(fingerprint):
                    result["sha256"] = fingerprint
                return result
    if (report.get("schema") == "image-secret-scan-v1" and report.get("status") == "ERROR"
            and report.get("complete") is False and type(report.get("exit_code")) is int
            and report["exit_code"] == 2):
        # The scanner report emits only fixed identifiers for failures. Keep a
        # bounded fingerprint, never the original identifier or exception text.
        code = report.get("error")
        if isinstance(code, str) and ERROR_IDENTIFIER.fullmatch(code):
            kind = {"scanner-operation-failed": "scan_operation_failed",
                    "classification-operation-failed": "classification_failed"}.get(
                        code, "scanner_reported_error")
            return {"kind": kind, "sha256": hashlib.sha256(code.encode("ascii")).hexdigest()}
    return {"kind": "diagnostic_unavailable"}


def project(report, outcome):
    result = {"schema": "image-scan-failure-v1", "outcome": outcome if isinstance(outcome, str) and outcome in OUTCOMES else "scan_refused",
              "complete": report.get("complete") is True}
    if result["outcome"] == "scanner_error" and not result["complete"]:
        result["diagnostic"] = incomplete_diagnostic(report)
    for field in ("archive_sha256",):
        value = report.get(field)
        if isinstance(value, str) and DIGEST.fullmatch(value):
            result[field] = value
    counts = report.get("counts", {})
    result["counts"] = {key: value for key, value in counts.items()
                        if key in {"detected", "blocking", "advisory", "reviewed", "allowlisted"}
                        and type(value) is int and 0 <= value <= 10**9} if isinstance(counts, dict) else {}
    findings = report.get("blocking_findings", report.get("findings", []))
    findings = findings if isinstance(findings, list) else []
    omitted = report.get("omitted_findings", 0)
    omitted = omitted if type(omitted) is int and 0 <= omitted <= 10**9 else 0
    result.update(findings=[], omitted_findings=omitted + max(0, len(findings) - 1000))
    for finding in findings[:1000]:
        if not isinstance(finding, dict):
            continue
        rule = finding.get("rule")
        row = {"rule": rule if isinstance(rule, str) and rule in RULES else "unrecognized_rule"}
        if isinstance(rule, str):
            row["rule_sha256"] = hashlib.sha256(rule.encode("utf-8", "surrogatepass")).hexdigest()
        for key in ("finding_id", "candidate_sha256", "file_sha256", "rule_sha256"):
            value = finding.get(key)
            if isinstance(value, str) and DIGEST.fullmatch(value):
                row[key] = value
        path = finding.get("path")
        if isinstance(path, str) and re.fullmatch(r"path-sha256:[a-f0-9]{64}", path):
            row["path"] = path
        if isinstance(finding.get("scope"), str) and finding["scope"] in {"layer", "config", "archive"}:
            row["scope"] = finding["scope"]
        for key in ("image", "layer", "entry", "line"):
            value = finding.get(key)
            if type(value) is int and -1 <= value <= 10**9:
                row[key] = value
        result["findings"].append(row)
    return result


def read(path, outcome):
    try:
        with path.open("rb") as stream:
            raw = stream.read(64 * 1024 * 1024 + 1)
        if len(raw) > 64 * 1024 * 1024:
            raise ValueError
        report = json.loads(raw)
        if not isinstance(report, dict):
            raise ValueError
        return project(report, outcome)
    except (OSError, ValueError, TypeError):
        return project({}, outcome)
