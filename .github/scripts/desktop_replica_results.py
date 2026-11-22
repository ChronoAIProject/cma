"""Validate first-attempt desktop results without claiming a performance budget."""
from collections import Counter
import json
import math
import os
import re
import xml.etree.ElementTree as ET

BOUNDARY = "authenticated public discovery through first returned synthetic echo frame"
OUTCOMES = {"ok", "discovery_transport", "discovery_refusal", "descriptor_body",
            "descriptor_shape", "websocket_refusal_or_transport", "frame_send",
            "frame_reply", "frame_deadline", "attempt_deadline"}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def json_value(text):
    def unique(pairs):
        value = {}
        for key, item in pairs:
            require(key not in value, "Duplicate JSON field")
            value[key] = item
        return value
    def nonfinite(_):
        raise ValueError("Nonfinite JSON number")
    return json.loads(text, object_pairs_hook=unique, parse_constant=nonfinite)


def records(output, name):
    result = []
    for line in output.splitlines():
        line = line.removeprefix(f"test {name} ... ")
        if line.startswith("DESKTOP_"):
            marker, separator, payload = line.partition("=")
            require(separator and marker in {
                "DESKTOP_PUBLIC_START", "DESKTOP_PUBLIC_SAMPLE", "DESKTOP_PUBLIC_MEASUREMENT",
                "DESKTOP_FIXTURE_ROOT", "DESKTOP_FIXTURE_CHILD", "DESKTOP_FIXTURE_CLEANUP",
            }, "Unknown or incomplete desktop record")
            value = json_value(payload)
            require(isinstance(value, dict), "Desktop record must be an object")
            result.append((marker, value))
    return result


def fixture_cleanup(rows):
    roots = [value for marker, value in rows if marker == "DESKTOP_FIXTURE_ROOT"]
    children = [value for marker, value in rows if marker == "DESKTOP_FIXTURE_CHILD"]
    completed = [value for marker, value in rows if marker == "DESKTOP_FIXTURE_CLEANUP"]
    require(len(roots) == 2 and len(completed) == 1, "Missing or duplicate owned cleanup")
    identity = roots[0].get("id")
    require(isinstance(identity, str) and re.fullmatch(r"receiver-[0-9a-f]{32}", identity),
            "Invalid owned receiver identity")
    require([row.get("event") for row in roots] == ["created", "removed"], "Receiver root cleanup incomplete")
    for row in roots + children + completed:
        require(type(row.get("schema")) is int and row["schema"] == 1 and row.get("id") == identity, "Cleanup identity/schema mismatch")
    started = {}
    exited = set()
    for row in children:
        pid = row.get("pid")
        require(type(pid) is int and pid > 0, "Missing owned child PID")
        if row.get("event") == "started":
            require(pid not in started, "Duplicate child start")
            started[pid] = row
        else:
            require(row.get("event") == "exited" and pid in started and pid not in exited,
                    "Unmatched or duplicate child exit")
            expected = row.get("expected")
            require((expected == "success" and row.get("success") is True and row.get("code") == 0)
                    or (expected == "forced" and row.get("success") is False and row.get("code") != 0),
                    "Unexpected child exit")
            exited.add(pid)
    require(started and set(started) == exited, "Owned child exit is unconfirmed")
    for pid in started:
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            continue
        except PermissionError:
            pass
        raise ValueError("Owned receiver PID still exists; preserve it for inspection")
    database = completed[0]
    for field in ("before", "after", "owned"):
        values = database.get(field)
        require(isinstance(values, list) and all(isinstance(value, str) and re.fullmatch(r"[0-9a-f]{64}", value) for value in values),
                "Missing database inventory hashes")
        require(len(values) == len(set(values)), "Duplicate database identity")
    require(len(database["owned"]) == 2 and not set(database["owned"]) & set(database["before"]),
            "Fixture databases were not independently owned")
    require(database["before"] == database["after"] and database.get("owned_absent") is True,
            "Owned database cleanup or preexisting inventory preservation failed")
    markers = [marker for marker, _ in rows if marker.startswith("DESKTOP_FIXTURE_")]
    require(markers[0] == "DESKTOP_FIXTURE_ROOT" and markers[-2:] == ["DESKTOP_FIXTURE_ROOT", "DESKTOP_FIXTURE_CLEANUP"],
            "Cleanup order is incomplete")
    return {"id": identity, "children": len(started), "database_inventory_preserved": True}


def cleanup(rows, name):
    fixtures = [(marker, value) for marker, value in rows if marker.startswith("DESKTOP_FIXTURE_")]
    identities = [value.get("id") for marker, value in fixtures
                  if marker == "DESKTOP_FIXTURE_ROOT" and value.get("event") == "created"]
    expected = 2 if name.endswith("::retained_legacy_rows_block_until_close_or_expiry_and_cannot_release_successor") else 1
    require(all(isinstance(value, str) for value in identities), "Invalid receiver identity")
    require(len(identities) == expected and len(set(identities)) == expected,
            "Missing or duplicate owned fixture")
    groups = [[row for row in fixtures if row[1].get("id") == identity] for identity in identities]
    require([row for group in groups for row in group] == fixtures,
            "Unmatched or interleaved fixture cleanup")
    return [fixture_cleanup(group) for group in groups]


def measurement(rows, mode, diagnostics):
    public = [(marker, value) for marker, value in rows if marker.startswith("DESKTOP_PUBLIC_")]
    count = 45 if diagnostics else 315
    require(len(public) == count * 2 + 1 and public[-1][0] == "DESKTOP_PUBLIC_MEASUREMENT",
            "Missing, duplicate or truncated public measurement")
    samples = []
    errors = Counter()
    width = 15 if diagnostics else 105
    for index in range(count):
        start_marker, start = public[index * 2]
        sample_marker, sample = public[index * 2 + 1]
        window, ordinal = divmod(index, width)
        require(start_marker == "DESKTOP_PUBLIC_START" and sample_marker == "DESKTOP_PUBLIC_SAMPLE",
                "Public start/sample order changed")
        require(type(start.get("window")) is int and type(start.get("ordinal")) is int
                and start == {"mode": mode, "window": window, "ordinal": ordinal}, "Wrong start mode/window/ordinal")
        require(type(sample.get("window")) is int and type(sample.get("ordinal")) is int
                and (sample["window"], sample["ordinal"]) == (window, ordinal), "Duplicate or missing sample identity")
        require(sample.get("warmup") is (ordinal < 5) and sample.get("cold") is (index == 0), "Wrong warmup/cold marker")
        require(type(sample.get("ingress")) is int and sample["ingress"] == int(mode == "two" and ordinal % 2 == 1),
                "Wrong measurement ingress")
        elapsed = sample.get("milliseconds")
        require(type(elapsed) in (int, float) and math.isfinite(elapsed) and elapsed >= 0, "Invalid elapsed sample")
        require(sample.get("outcome") in OUTCOMES, "Unknown sample outcome")
        phase_keys = {"discovery_original_resolver", "websocket_original_resolver", "provider"}
        require((phase_keys <= sample.keys()) if diagnostics else not (phase_keys & sample.keys()),
                "Diagnostic and acceptance samples were mixed")
        if sample["outcome"] != "ok":
            errors[sample["outcome"]] += 1
        samples.append(sample)
    summary = public[-1][1]
    require(type(summary.get("schema")) is int and summary["schema"] == 1 and summary.get("mode") == mode
            and summary.get("diagnostics") is diagnostics and summary.get("boundary") == BOUNDARY,
            "Measurement schema/mode/boundary mismatch")
    require(summary.get("samples") == samples, "Aggregate does not retain the exact raw samples")
    require(diagnostics or not errors, "Normal measurement contains non-ok samples")
    return {"samples": count, "warmup": 15, "analysis_samples": count - 15, "outcomes": dict(errors),
            "diagnostics": diagnostics}


def validate(path, names, mode, diagnostics):
    try:
        root = ET.parse(path).getroot()
    except ET.ParseError as error:
        raise ValueError("Truncated or malformed result document") from error
    require(root.tag == "testsuites", "Unexpected result document")
    for field, expected in (("tests", len(names)), ("failures", 0), ("errors", 0), ("skipped", 0)):
        require(root.get(field) == str(expected), f"Unexpected result {field}")
    suites = root.findall("testsuite")
    require(len(suites) == 1 and suites[0].get("name") == "cma::bin/cma", "Unexpected executed binary")
    for field in ("tests", "failures", "errors", "skipped"):
        require(suites[0].get(field) == root.get(field), "Suite totals disagree")
    cases = suites[0].findall("testcase")
    require(Counter(case.get("name") for case in cases) == Counter(names), "Missing, duplicate or unexpected parent result")
    result = {"executed": [], "cleanup": []}
    for case in cases:
        require(case.get("classname") == "cma::bin/cma", "Result binary identity mismatch")
        require(all(child.tag in {"system-out", "system-err"} for child in case), "Failed, skipped or retried parent")
        output = case.findall("system-out")
        require(len(output) == 1 and output[0].text, "Missing retained parent output")
        rows = records(output[0].text, case.get("name"))
        result["cleanup"].extend(cleanup(rows, case.get("name")))
        if mode != "contracts":
            result["measurement"] = measurement(rows, mode, diagnostics)
        else:
            require(not any(marker.startswith("DESKTOP_PUBLIC_") for marker, _ in rows), "Measurement entered correctness run")
        result["executed"].append(case.get("name"))
    require(len({item["id"] for item in result["cleanup"]}) == len(result["cleanup"]), "Reused fixture identity")
    return result


def cleanup_receipts(path, names):
    result = []
    try:
        root = ET.parse(path).getroot()
        for case in root.findall("./testsuite/testcase"):
            name = case.get("name")
            if name not in names:
                continue
            record = {"test": name, "verified": False}
            try:
                record["details"] = cleanup(records(case.findtext("system-out") or "", name), name)
                record["verified"] = True
            except (ValueError, OSError, TypeError, KeyError) as error:
                record["error"] = str(error)
            result.append(record)
    except (OSError, ET.ParseError) as error:
        result.append({"verified": False, "error": type(error).__name__})
    return result
