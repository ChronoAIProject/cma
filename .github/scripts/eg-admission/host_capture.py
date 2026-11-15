"""Parent-owned host capture after confirmed process-group exit, before disposal."""
import json
from pathlib import Path

from observations import collect, checked_hash

MAX_TRACE = 64 * 1024 * 1024


def capture(raw, identity):
    packet = collect(raw, identity)
    packet["host_capture_sha256"] = packet.pop("pod_uid_sha256")
    packet["capture_kind"] = "local_child_process"
    return packet


def bounded_read(path, maximum):
    if path.is_symlink() or not path.is_file():
        raise ValueError("capture_file_missing")
    with path.open("rb") as handle:
        raw = handle.read(maximum + 1)
    if len(raw) > maximum:
        raise ValueError("capture_file_oversized")
    return raw


def checked_metadata(raw, executable):
    metadata = json.loads(raw)
    if set(metadata) != {"replicas", "enabled", "ordinal", "process_sha256", "executable_sha256", "joined"}:
        raise ValueError("capture_metadata_fields")
    if type(metadata["replicas"]) is not int or metadata["replicas"] not in (1, 2):
        raise ValueError("capture_metadata_replicas")
    if type(metadata["ordinal"]) is not int or not 1 <= metadata["ordinal"] <= metadata["replicas"]:
        raise ValueError("capture_metadata_ordinal")
    if type(metadata["enabled"]) is not bool or type(metadata["joined"]) is not bool:
        raise ValueError("capture_metadata_boolean")
    checked_hash(metadata["process_sha256"])
    if metadata["executable_sha256"] != checked_hash(executable):
        raise ValueError("capture_executable_mismatch")
    return metadata


def collect_directory(temporary, evidence, executable):
    """Caller must establish group absence. On failure it must retain temporary."""
    rows, failures = [], []
    directories = sorted(temporary.glob("cmaeg-runner-*"))
    if not directories or len(directories) > 6:
        failures.append("capture_directory_count")
    for directory in directories[:6]:
        try:
            if directory.is_symlink() or not directory.is_dir():
                raise ValueError("capture_directory_invalid")
            metadata = checked_metadata(bounded_read(directory / "capture.json", 4096), executable)
            raw = bounded_read(directory / "trace.log", MAX_TRACE)
            packet = capture(raw, metadata["process_sha256"])
            packet["host_process"] = metadata
            packet["group_exit_confirmed"] = True
            name = "n{}-{}-{}.json".format(metadata["replicas"], str(metadata["enabled"]).lower(), metadata["ordinal"])
            with (evidence / name).open("x") as handle:
                json.dump(packet, handle, sort_keys=True)
                handle.write("\n")
            rows.append({"file": name, "rejected_records": packet["rejected_records"], "records": len(packet["observations"])})
            if packet["rejected_records"]:
                failures.append("capture_rejected_records")
            if metadata["enabled"] and not any(row["event"] == "eg_admission_attempt" for row in packet["observations"]):
                failures.append("capture_missing_attempt")
        except (OSError, ValueError, TypeError, KeyError):
            # Do not export path, parser exception, raw fields or arbitrary log text.
            failures.append("capture_child_failed")
    receipt = {"complete": not failures, "captures": rows, "failures": failures}
    with (evidence / "capture-receipt.json").open("x") as handle:
        json.dump(receipt, handle, sort_keys=True)
        handle.write("\n")
    return receipt
