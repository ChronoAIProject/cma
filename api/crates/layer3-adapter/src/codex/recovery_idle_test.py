"""Idle reopen uses real history/home evidence and refuses every unsettled turn."""
import copy
import os
import json
import unittest
from unittest.mock import patch

from recovery_test import AGENT, Fixture, line, m, process, SOURCE, TURN

exec(compile((SOURCE / "recovery_ledger.py").read_text(), "recovery_ledger", "exec"), m.__dict__)


class IdleReopen(Fixture):
    def setUp(self):
        super().setUp()
        self.supervisor = {**process(1, 0), "sid": 1}
        self.table = {1: self.supervisor, os.getpid(): process(os.getpid(), 0)}
        self.request = {"version": 1, "action": "inspect_idle", "runtime": self.runtime, "agents": [AGENT]}
        real_readlink = m.os.readlink
        patch.object(m.os, "readlink", side_effect=lambda path, *a, **kw:
                     "/usr/local/bin/layer4-runtime" if path == "/proc/1/exe" else real_readlink(path, *a, **kw)).start()
        patch.object(m, "inventory", side_effect=lambda: copy.deepcopy(self.table)).start()
        patch.object(m, "proc_identity", side_effect=lambda pid: copy.deepcopy(self.supervisor)).start()
        patch.object(m, "services", return_value=[]).start()
        patch.object(m, "environment", return_value={}).start()

    def inspect(self):
        m.validate(self.request)
        return m.inspect(self.request)

    def ledger(self):
        operation = "agent:" + AGENT["agentId"] + ":home:create:v1"
        common = {"version": 1, "sandbox_id": self.runtime["sandboxId"],
                  "operation_id": operation, "request_sha256": "d" * 64,
                  "payload_sha256": "e" * 64,
                  "subject": {"kind": "create_agent_home", "agent_id": AGENT["agentId"]}}
        journal = {**common, "status": "completed", "receipt": {
            "operation_id": operation, "request_sha256": common["request_sha256"],
            "agent_id": AGENT["agentId"], "path": str(self.home)}}
        authority = {**common, "authority_fence": 1, "disposition": "present",
                     "resource": {"kind": "agent_home", "agent_id": AGENT["agentId"]}}
        jpath = self.state / "runtime-mutations" / (m.sha(operation.encode()) + ".json")
        apath = self.state / "mutation-authority" / m.ledger_authority_name(authority)
        for path, record in ((jpath, journal), (apath, authority)):
            path.parent.mkdir(exist_ok=True)
            path.write_text(json.dumps(record))
        return jpath, apath

    def test_settled_original_history_is_read_only_without_any_quiescence(self):
        self.terminal()
        self.ledger()
        before = {p: p.read_bytes() for p in self.root.rglob("*") if p.is_file()}
        with patch.object(m, "send_eof", side_effect=AssertionError("must not signal")), \
             patch.object(m, "write_journal", side_effect=AssertionError("must not write")):
            result = self.inspect()
        self.assertTrue(result["plan"]["idleReopen"])
        self.assertEqual(result["plan"]["idleLedger"]["journals"], 1)
        self.assertEqual(result["plan"]["agents"], [AGENT])
        self.assertEqual(result["planSha256"], m.sha(m.encode(result["plan"])))
        self.assertEqual(before, {p: p.read_bytes() for p in self.root.rglob("*") if p.is_file()})
        self.assertFalse((self.root / ".cma/recovery").exists())

    def test_terminal_failure_pending_receipt_or_missing_outcome_remains_held(self):
        self.terminal()
        jpath, _ = self.ledger()
        completed = json.loads(jpath.read_text())
        for status in ("started", "failed", "indeterminate"):
            jpath.write_text(json.dumps({**completed, "status": status}))
            with self.subTest(status=status), self.assertRaisesRegex(m.Refused, "ledger_unsettled"):
                self.inspect()
        jpath.unlink()
        with self.assertRaisesRegex(m.Refused, "ledger_unsettled"):
            self.inspect()

    def test_used_current_process_scope_cannot_be_rearmed_with_new_open_key(self):
        self.terminal()
        jpath, _ = self.ledger()
        for folder in ("runtime-mutations", "mutation-authority"):
            current = self.state / "process-generation-1" / folder
            current.mkdir(parents=True, exist_ok=True)
            retained = current / jpath.name
            retained.write_bytes(jpath.read_bytes())
            with self.subTest(folder=folder), self.assertRaisesRegex(m.Refused, "ledger_process_scope_used"):
                self.inspect()
            retained.unlink()

    def test_completed_journal_without_matching_authority_refuses(self):
        self.terminal()
        _, apath = self.ledger()
        apath.unlink()
        with self.assertRaisesRegex(m.Refused, "ledger_unsettled"):
            self.inspect()

    def test_ledger_byte_budget_is_enforced_while_reading(self):
        self.terminal()
        self.ledger()
        with patch.object(m, "LEDGER_BYTES", 1):
            with self.assertRaisesRegex(m.Refused, "ledger_capacity"):
                self.inspect()

    def test_indexed_partial_and_temporary_formats_are_unproved(self):
        self.terminal()
        for name in ("mutation-ledger-v2.current.json", "mutation-ledger-v2.lock", ".mutation-ledger-v2.preparing"):
            path = self.state / name
            path.touch()
            with self.subTest(name=name), self.assertRaisesRegex(m.Refused, "ledger_format_unproved"):
                self.inspect()
            path.unlink()
        jpath, _ = self.ledger()
        (jpath.parent / ".mutation-journal-tmp.tmp").touch()
        with self.assertRaisesRegex(m.Refused, "ledger_unproved"):
            self.inspect()

    def test_capacity_limit_and_foreign_or_symlink_ledger_refuse(self):
        self.terminal()
        jpath, _ = self.ledger()
        record = json.loads(jpath.read_text())
        jpath.write_text(json.dumps({**record, "sandbox_id": "foreign"}))
        with self.assertRaisesRegex(m.Refused, "ledger_unproved"):
            self.inspect()
        jpath.unlink()
        jpath.symlink_to(self.history)
        with self.assertRaises(OSError):
            self.inspect()
        jpath.unlink()
        for i in range(129):
            (jpath.parent / (format(i, "064x") + ".json")).touch()
        with self.assertRaisesRegex(m.Refused, "ledger_capacity"):
            self.inspect()

    def test_ledger_change_during_history_is_not_admitted(self):
        self.terminal()
        _, apath = self.ledger()
        original = m.history_summary

        def history(*args):
            result = original(*args)
            apath.write_text(apath.read_text() + " ")
            return result

        with patch.object(m, "history_summary", side_effect=history):
            with self.assertRaisesRegex(m.Refused, "ledger_changed"):
                self.inspect()

    def test_old_unfinished_turn_is_not_silently_treated_as_terminal(self):
        with self.assertRaisesRegex(m.Refused, "native_turn_unsettled"):
            self.inspect()
        self.assertEqual(self.history.read_bytes(), self.initial)

    def test_unrelated_process_prevents_admission(self):
        self.terminal()
        self.table[123456] = process(123456, 1)
        with patch.object(m.os, "readlink", return_value="/unrelated/program"):
            with self.assertRaises(m.Refused):
                self.inspect()

    def test_process_appearing_after_history_prevents_admission(self):
        self.terminal()
        original = m.history_summary

        def history(*args):
            result = original(*args)
            self.table[123456] = process(123456, 1)
            return result

        with patch.object(m, "history_summary", side_effect=history):
            with self.assertRaisesRegex(m.Refused, "untracked_process"):
                self.inspect()

    def test_boot_replacement_during_history_refuses(self):
        self.terminal()
        original = m.history_summary

        def history(*args):
            result = original(*args)
            self.writer["boot_id"] = "e" * 32
            self.publish_boot()
            return result

        with patch.object(m, "history_summary", side_effect=history):
            with self.assertRaisesRegex(m.Refused, "boot_changed"):
                self.inspect()

    def test_history_change_between_passes_refuses_even_when_well_formed(self):
        self.terminal()
        original = m.history_summary
        reads = 0

        def history(*args):
            nonlocal reads
            reads += 1
            if reads == 2:
                with self.history.open("ab") as file:
                    file.write(line("event_msg", {"type": "task_started", "turn_id": TURN}))
            return original(*args)

        with patch.object(m, "history_summary", side_effect=history):
            with self.assertRaisesRegex(m.Refused, "history_prefix_changed"):
                self.inspect()


if __name__ == "__main__":
    unittest.main()
