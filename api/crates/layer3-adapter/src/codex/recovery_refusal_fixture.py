"""Emit pinned-provider refusals from disposable history and proc fixtures."""
import io
import json
from pathlib import Path
import sys
import tempfile
import types
from unittest.mock import patch
import uuid

m = types.ModuleType("recovery_refusal_fixture")
exec(compile(sys.stdin.read(), "pinned-recovery", "exec"), m.__dict__)
AGENT = {"agentId": "agt_" + "a" * 32,
         "conversationId": "11111111-1111-7111-8111-111111111111"}
TURN = "22222222-2222-7222-8222-222222222222"


def line(kind, payload):
    return json.dumps({"type": kind, "payload": payload}).encode() + b"\n"


def provider_response(operation, invalid_request=False):
    request = {"version": 1, "action": "inspect", "agents": [AGENT],
               "runtime": {"sandboxId": "sbx_fixture", "homeIdentity": "home:fixture",
                           "generation": 1, "bootId": "b" * 32}}
    if invalid_request:
        request["runtime"]["generation"] = 0
    output = io.BytesIO()
    with patch.object(m.sys, "stdin", types.SimpleNamespace(buffer=io.BytesIO(m.encode(request)))), \
         patch.object(m.sys, "stdout", types.SimpleNamespace(buffer=output)), \
         patch.object(m, "inspect", lambda _: operation()), \
         patch.object(m.signal, "signal"), patch.object(m.signal, "alarm"):
        # Exercise the actual request validator, exception mapping and response
        # serialization; only unrelated Linux process discovery is replaced.
        m.main()
    return json.loads(output.getvalue())


def run_case(case):
    with tempfile.TemporaryDirectory() as temporary:
        root = Path(temporary).resolve()
        home = root / ".cma/agents" / AGENT["agentId"] / "home"
        history = home / "sessions/original.jsonl"
        history.parent.mkdir(parents=True)
        initial = line("session_meta", {"id": AGENT["conversationId"], "cli_version": "0.147.0"})
        history.write_bytes(initial)
        with patch.dict(m.__dict__, HOME=str(root), PROC=str(root / "proc"), DEADLINE=float("inf")):
            operation = lambda: m.history_summary(AGENT)
            if case == "missing_original":
                history.write_bytes(line("session_meta", {"id": TURN}))
            elif case == "metadata_absent":
                history.write_bytes(line("event_msg", {}))
            elif case == "metadata_malformed":
                history.write_bytes(line("session_meta", {"id": "invalid"}))
            elif case == "metadata_duplicate":
                history.write_bytes(initial + initial)
            elif case == "conversation_duplicate":
                (history.parent / "duplicate.jsonl").write_bytes(initial)
            elif case == "history_symlink":
                outside = root / "outside.jsonl"
                history.rename(outside)
                history.symlink_to(outside)
            elif case == "history_path":
                history.rename(history.parent / "unsafe name.jsonl")
            elif case == "history_empty":
                history.write_bytes(b"")
            elif case == "history_file_limit":
                for index in range(128):
                    (history.parent / f"extra-{index}.jsonl").write_bytes(
                        line("session_meta", {"id": str(uuid.UUID(int=index + 1))}))
            elif case == "history_incomplete":
                history.write_bytes(initial + b'{"type":')
            elif case == "history_invalid":
                history.write_bytes(b"[]\n")
            elif case == "evidence_io":
                operation = lambda: m.history_summary({**AGENT, "agentId": "agt_" + "c" * 32})
            elif case == "evidence_decode":
                history.write_bytes(b"{invalid}\n")
            elif case == "evidence_type":
                history.write_bytes(line("session_meta", {"id": []}))
            elif case in ("history_missing", "history_replaced", "history_prefix_changed"):
                previous = m.histories(AGENT)
                if case == "history_missing":
                    history.unlink()
                elif case == "history_replaced":
                    history.rename(root / "old.jsonl")
                    history.write_bytes(initial)
                else:
                    history.write_bytes(initial.replace(b"0.147.0", b"0.146.0"))
                operation = lambda: m.history_summary(AGENT, previous)
            elif case == "history_descriptor":
                proc = root / "proc"
                (proc / "10/fd").mkdir(parents=True)
                (proc / "stat").write_text("btime 100\n")
                (proc / "uptime").write_text("0 0\n")
                (proc / "10/fd/4").symlink_to(home / "sessions/unrecognized.txt")

                def operation():
                    with patch.object(m.time, "time", return_value=100):
                        return m.native_history_clock(10, str(home), {"start": 100})
            elif case == "turn_identity":
                history.write_bytes(initial + line("event_msg", {"type": "task_started", "turn_id": "invalid"}))
            elif case == "turn_limit":
                history.write_bytes(initial + b"".join(
                    line("event_msg", {"type": "task_started", "turn_id": str(uuid.UUID(int=i + 1))})
                    for i in range(129)))
            elif case in ("native_turn_unsettled", "native_turn_restarted"):
                started = line("event_msg", {"type": "task_started", "turn_id": TURN})
                history.write_bytes(initial + started)
                previous = m.history_summary(AGENT)
                if case == "native_turn_restarted":
                    previous["files"][0]["historicalOpenTurns"] = [TURN]
                    history.write_bytes(initial + started + started)
                plan = {"agents": [AGENT], "histories": [previous], "topology": []}

                def operation():
                    with patch.object(m, "inventory", return_value={}), \
                         patch.object(m, "absent", return_value=True):
                        return m.final_history(plan)
            elif case != "request_invalid":
                raise AssertionError(case)
            return provider_response(operation, case == "request_invalid")


expected = {
    "missing_original": "history_identity",
    "metadata_absent": "history_identity",
    "metadata_malformed": "history_identity",
    "metadata_duplicate": "history_identity",
    "conversation_duplicate": "history_identity",
    "history_symlink": "history_symlink",
    "history_path": "history_path",
    "history_empty": "history_limit",
    "history_file_limit": "history_limit",
    "history_incomplete": "history_incomplete",
    "history_invalid": "history_invalid",
    "evidence_io": "evidence_unavailable",
    "evidence_decode": "evidence_unavailable",
    "evidence_type": "evidence_unavailable",
    "history_missing": "history_missing",
    "history_replaced": "history_replaced",
    "history_prefix_changed": "history_prefix_changed",
    "history_descriptor": "history_descriptor",
    "turn_identity": "turn_identity",
    "turn_limit": "turn_limit",
    "native_turn_unsettled": "native_turn_unsettled",
    "native_turn_restarted": "native_turn_restarted",
    "request_invalid": "request_invalid",
}
responses = {}
for case, code in expected.items():
    response = run_case(case)
    assert response == {"version": 1, "status": "refused", "code": code}, (case, response)
    responses[case] = response
print(json.dumps(responses))
