"""Retain fixed diagnostic boundaries without interpreting them as acceptance."""
import re


def phase_observations(output):
    stages = []
    per_process = {"ready", "ingress", "headers", "body", "exit"}
    for count, enabled, phase, state, ordinal, elapsed in re.findall(
            rb"(?<![\w])replica_stage replicas=([123]) ownership=(true|false) "
            rb"phase=(cohort|setup|migration|ready|ingress|headers|body|oracle_open|rejected_oracle|oracle|exit|reconcile|database_cleanup|teardown) "
            rb"state=(start|end) ordinal=([0-3]) elapsed_ms=([0-9]{1,9})(?=\r?\n|$)", output):
        count, ordinal, phase = int(count), int(ordinal), phase.decode()
        if (phase in per_process and not 1 <= ordinal <= count) or (phase not in per_process and ordinal != 0):
            continue
        stages.append({"replicas": count, "enabled": enabled == b"true", "phase": phase,
                       "boundary": state.decode(), "process_ordinal": ordinal,
                       "cohort_elapsed_ms": int(elapsed)})
    headers = []
    for count, enabled, ordinal, status, elapsed in re.findall(
            rb"(?<![\w])replica_http replicas=([123]) ownership=(true|false) "
            rb"ordinal=([1-3]) status=([1-5][0-9]{2}) elapsed_ms=([0-9]{1,9})(?=\r?\n|$)", output):
        if int(ordinal) <= int(count):
            headers.append({"replicas": int(count), "enabled": enabled == b"true",
                            "process_ordinal": int(ordinal), "status": int(status),
                            "cohort_elapsed_ms": int(elapsed)})
    # An unmatched start is censored. No duration, cohort completion, receipt or
    # cross-process timing is inferred; HTTP headers do not prove durable custody.
    return {"cohort_stage_observations": stages, "ingress_header_observations": headers}
