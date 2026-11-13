"""Allowlist complete tracing records; preserve nesting and source order.

This parser reports observations only. It does not infer custody, rollback,
durations across different clock origins, or application/provider completion.
Raw input is hashed in memory and is never copied into the returned packet.
"""

import hashlib
import json
import re
import uuid

from observation_schema import STAGES, exact_keys, validate_event

HASH = re.compile(r"[0-9a-f]{64}\Z")
FIELD = re.compile(r'([a-zA-Z_][a-zA-Z0-9_]*)=("(?:[^"\\\r\n]|\\.)*"|[a-zA-Z0-9_]+)')
SPAN = re.compile(r"([a-zA-Z_][a-zA-Z0-9_.]*)\{")
PREFIX = re.compile(
    r"(?:\d{4}-\d\d-\d\dT[0-9:.]+Z\s+)?(?:TRACE|DEBUG|INFO|WARN|ERROR)\s+"
)
MESSAGES = {
    "cmaeg::ingress::admission_trace": (
        "Admission observation",
        "Admission returned a refusal",
    ),
    "cmaeg::ingress::routed::problem": ("Admission error mapped to public refusal",),
    "cmaeg::records::mongo::replica_observation": ("replica persistence observation",),
}


def digest(value):
    return hashlib.sha256(value).hexdigest()


def checked_hash(value):
    if not isinstance(value, str) or not HASH.fullmatch(value):
        raise ValueError("invalid observation digest")
    return value


def fields(text, stop=None):
    result = {}
    while text:
        text = text.lstrip(" ")
        if stop and text.startswith(stop):
            return result, text[len(stop) :]
        if not text:
            break
        match = FIELD.match(text)
        if not match or match[1] in result:
            raise ValueError("invalid or repeated observation field")
        key, raw = match[1], match[2]
        if len(raw) > 1024:
            raise ValueError("oversized observation field")
        if raw.startswith('"'):
            value = json.loads(raw)
        elif raw in ("true", "false"):
            value = raw == "true"
        elif raw.isdigit():
            value = int(raw)
        else:
            value = raw
        result[key] = value
        text = text[match.end() :]
        if text and not text.startswith(" ") and not (stop and text.startswith(stop)):
            raise ValueError("invalid observation field framing")
    if stop:
        raise ValueError("unterminated observation span")
    return result, ""


def parse_line(line):
    if not line.endswith("\n") or len(line) > 32768 or "\x1b" in line:
        raise ValueError("incomplete or oversized observation record")
    line = line.removesuffix("\n").removesuffix("\r").lstrip(" ")
    prefix = PREFIX.match(line)
    if not prefix:
        raise ValueError("invalid tracing prefix")
    line = line[prefix.end() :]
    spans = []
    while match := SPAN.match(line):
        values, line = fields(line[match.end() :], "}")
        if not line.startswith(":") or len(spans) >= 16:
            raise ValueError("invalid span chain")
        spans.append((match[1], values))
        line = line[1:].lstrip(" ")
    target, separator, tail = line.partition(": ")
    if not separator or target not in MESSAGES:
        raise ValueError("unapproved observation target")
    message = next(
        (value for value in MESSAGES[target] if tail.startswith(value + " ")), None
    )
    if message is None:
        raise ValueError("unapproved observation message")
    event, _ = fields(tail[len(message) :])
    validate_event(event)
    if (event["event"] == "eg_admission_record") != (
        target == "cmaeg::records::mongo::replica_observation"
    ):
        raise ValueError("event target mismatch")
    if (event["event"] == "eg_admission_public_mapping") != (
        target == "cmaeg::ingress::routed::problem"
    ):
        raise ValueError("mapping target mismatch")
    attempts = [value for name, value in spans if name == "eg_admission_attempt"]
    requests = [value for name, value in spans if name == "cmaeg.request"]
    if len(attempts) > 1 or len(requests) > 1:
        raise ValueError("ambiguous admission identity")
    identity_fields = ("attempt_sha256", "process_sha256", "admission_sha256")
    attempt = dict.fromkeys(identity_fields)
    if attempts:
        attempt = attempts[0]
        exact_keys(attempt, identity_fields)
        for value in attempt.values():
            checked_hash(value)
    elif event["event"] in ("eg_admission_attempt", "eg_admission_stage"):
        raise ValueError("missing admission identity")
    operations = []
    for name, value in spans:
        if name == "eg_admission_operation":
            exact_keys(value, ("stage", "operation_sha256"))
            if value["stage"] not in STAGES:
                raise ValueError("unapproved admission stage")
            checked_hash(value["operation_sha256"])
            operations.append(value)
    if event["event"] == "eg_admission_stage" and not operations:
        raise ValueError("missing stage identity")
    if operations and not attempts:
        raise ValueError("operation ancestry lacks its admission identity")
    request_sha256 = None
    if requests:
        request_id = requests[0].get("request_id")
        parsed = uuid.UUID(request_id)
        if parsed.version != 4 or str(parsed) != request_id:
            raise ValueError("noncanonical synthetic request ID")
        request_sha256 = digest(request_id.encode())
    return {
        **attempt,
        "request_sha256": request_sha256,
        "operations": operations,
        **event,
    }


def collect(raw, pod_uid_sha256):
    """One bounded pod-log capture; caller independently binds pod UID and image."""
    checked_hash(pod_uid_sha256)
    if not isinstance(raw, bytes) or len(raw) > 64 * 1024 * 1024:
        raise ValueError("invalid capture size")
    retained = []
    rejected = 0
    for ordinal, line in enumerate(raw.splitlines(keepends=True), 1):
        # Other application logs may contain private data; never retain them.
        if b'event="eg_admission_' not in line:
            continue
        try:
            event = parse_line(line.decode("utf-8"))
        except (ValueError, TypeError, AttributeError, UnicodeError):
            rejected += 1
            continue
        retained.append({"source_ordinal": ordinal, **event})
    return {
        "schema": "eg_admission_observations_v1",
        "pod_uid_sha256": pod_uid_sha256,
        "input_sha256": digest(raw),
        "input_bytes": len(raw),
        "rejected_records": rejected,
        "observations": retained,
    }
