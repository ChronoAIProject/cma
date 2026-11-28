"""Offline recovery evidence: real files/journal plus controlled Linux processes."""
import copy
import json
import os
from pathlib import Path
import signal
import tempfile
import time
import types
import unittest
from unittest.mock import patch

SOURCE = Path(__file__).parent
m = types.ModuleType("recovery_fixture")
exec(compile((SOURCE / "recovery_io.py").read_text() + "\n" + (SOURCE / "recovery.py").read_text(), "recovery", "exec"), m.__dict__)
AGENT = {"agentId": "agt_" + "a" * 32, "conversationId": "11111111-1111-7111-8111-111111111111"}
TURN = "22222222-2222-7222-8222-222222222222"
BOOT = "3" * 32
OP = "sandbox-lifecycle-v1:sbx_" + "b" * 32 + ":2"
JOURNAL = m.sha(OP.encode()) + ".json"


def line(kind, payload, timestamp=None):
    value = {"type": kind, "payload": payload}
    if timestamp is not None:
        value["timestamp"] = timestamp
    return json.dumps(value).encode() + b"\n"


def process(pid, parent, start=None):
    return {"pid": pid, "start": start or pid * 100, "ppid": parent, "sid": 20,
            "commandSha256": m.sha(str(pid).encode()), "exeSha256": m.sha(b"exe")}


class Fixture(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        # macOS /tmp is a symlink; production descriptor walks deliberately refuse it.
        self.root = Path(self.temp.name).resolve()
        self.addCleanup(patch.stopall)
        patch.dict(m.__dict__, HOME=str(self.root), STATE=str(self.root / "state"), DEADLINE=float("inf")).start()
        self.home = self.root / ".cma/agents" / AGENT["agentId"] / "home"
        self.history = self.home / "sessions/2026/10/28/rollout-2026-10-28T01-00-00-test.jsonl"
        self.history.parent.mkdir(parents=True)
        self.initial = line("session_meta", {"id": AGENT["conversationId"], "cli_version": "0.147.0"})
        self.initial += line("event_msg", {"type": "task_started", "turn_id": TURN})
        self.history.write_bytes(self.initial)
        self.runtime = {"sandboxId": "sbx_test", "homeIdentity": "home:test", "generation": 1, "bootId": BOOT}
        self.state = self.root / "state/sbx_test"
        (self.state / "home-binding").mkdir(parents=True)
        (self.state / "workspace-removal-writer-v1.lock").touch()
        self.writer = {"sandbox_id": "sbx_test", "home_identity": "home:test", "generation": 1, "boot_id": BOOT}
        self.publish_boot()
        (self.state / "home-binding/binding.json").write_text(json.dumps({
            "version": 2, "sandbox_id": "sbx_test", "home_identity": "home:test", "runtime_generation": 1, "restore": None}))

    def publish_boot(self):
        (self.state / "workspace-removal-writer-v1.json").write_text(json.dumps(self.writer))

    def terminal(self):
        with self.history.open("ab") as file:
            file.write(line("response_item", {"role": "assistant", "content": "completed output"}))
            file.write(line("event_msg", {"type": "turn_aborted", "turn_id": TURN}))


class Files(Fixture):
    def test_inspection_has_no_writes_or_contents(self):
        before = {p: p.read_bytes() for p in self.root.rglob("*") if p.is_file()}
        result = m.history_summary(AGENT)
        self.assertEqual(result["files"][0]["openTurns"], [TURN])
        self.assertEqual(before, {p: p.read_bytes() for p in self.root.rglob("*") if p.is_file()})
        self.assertNotIn("cli_version", m.encode(result).decode())
        self.assertFalse((self.root / ".cma/recovery").exists())

    def test_full_history_over_8mib_and_exact_old_prefix(self):
        with self.history.open("ab") as file:
            for _ in range(10):
                file.write(line("response_item", {"content": "x" * 1048576}))
        before = m.histories(AGENT)
        self.terminal()
        result = m.histories(AGENT, before)
        self.assertGreater(result[0]["bytes"], 8 * 1048576)
        self.assertEqual(result[0]["openTurns"], [])
        self.assertEqual(result[0]["lastTurn"], {"turnId": TURN, "type": "turn_aborted"})
        self.assertNotEqual(result[0]["sha256"], before[0]["sha256"])
        self.assertLess(len(m.encode(result)), 1024)

    def test_changed_prefix_is_refused_even_with_terminal_marker(self):
        before = m.histories(AGENT)
        self.history.write_bytes(self.initial.replace(b"0.147.0", b"0.146.0"))
        self.terminal()
        with self.assertRaisesRegex(m.Refused, "history_prefix_changed"):
            m.histories(AGENT, before)

    def test_partial_and_invalid_jsonl_are_refused(self):
        for tail in (b'{"type":', b'{invalid}\n'):
            self.history.write_bytes(self.initial + tail)
            with self.assertRaises((m.Refused, ValueError)):
                m.histories(AGENT)

    def test_missing_replaced_and_wrong_identity_are_refused(self):
        previous = m.histories(AGENT)
        self.history.unlink()
        with self.assertRaisesRegex(m.Refused, "history_missing"):
            m.histories(AGENT, previous)
        self.history.write_bytes(self.initial.replace(AGENT["conversationId"].encode(), TURN.encode()))
        with self.assertRaisesRegex(m.Refused, "history_identity"):
            m.histories(AGENT)

    def test_history_and_parent_symlinks_are_refused(self):
        outside = self.root / "outside.jsonl"
        self.history.rename(outside)
        self.history.symlink_to(outside)
        with self.assertRaisesRegex(m.Refused, "history_symlink"):
            m.histories(AGENT)
        self.history.unlink()
        self.history.parent.rmdir()
        self.history.parent.symlink_to(self.root, target_is_directory=True)
        with self.assertRaisesRegex(m.Refused, "history_symlink"):
            m.histories(AGENT)

    def test_request_bounds_and_exact_fields(self):
        good = {"version": 1, "action": "inspect", "runtime": self.runtime, "agents": [AGENT]}
        m.validate(good)
        m.validate({**good, "agents": []})
        for changes in ({"operationId": OP}, {"agents": None}, {"agents": {}}, {"agents": [AGENT] * 17}, {"action": "kill"}):
            with self.assertRaises(m.Refused):
                m.validate({**good, **changes})

    def test_boot_generation_and_home_changes_are_refused(self):
        with m.boot_guard(self.runtime):
            pass
        self.writer["boot_id"] = OP
        self.publish_boot()
        with self.assertRaisesRegex(m.Refused, "boot_changed"):
            with m.boot_guard(self.runtime):
                self.fail("stale boot admitted")

    def test_historical_turns_require_preboot_time_and_later_terminal_if_open(self):
        file = {"path": "sessions/old.jsonl", "openTurnStarts": {TURN: {"at": 1000, "ordinal": 2}},
                "lastTerminal": {"at": 2000, "ordinal": 3}}
        tree = {"nativeStartEpochMs": 100000, "openHistoryPaths": [file["path"]]}
        self.assertEqual(m.historical_turns(file, tree), [TURN])
        for terminal in (None, {"at": 500, "ordinal": 3}, {"at": 2000, "ordinal": 1}):
            self.assertEqual(m.historical_turns({**file, "lastTerminal": terminal}, tree), [])
        for at in (None, 50000, 100001):
            changed = {**file, "openTurnStarts": {TURN: {"at": at, "ordinal": 2}}}
            self.assertEqual(m.historical_turns(changed, tree), [])
        self.assertEqual(m.historical_turns({**file, "lastTerminal": None}, {**tree, "openHistoryPaths": []}), [TURN])


class Quiesce(Fixture):
    def setUp(self):
        super().setUp()
        self.supervisor = process(10, 1)
        self.bridge = process(20, 10)
        self.launcher = process(30, 20)
        self.native = process(40, 30)
        self.tool = process(50, 40)
        self.live = {p["pid"]: p for p in (self.supervisor, self.bridge, self.launcher, self.native, self.tool)}
        self.pipe = {"reader": "pipe:[123]", "writer": "pipe:[123]"}
        self.topology = {"agentId": AGENT["agentId"], "bridgePid": 20, "launcherPid": 30, "nativePid": 40,
                         "processes": [self.bridge, self.launcher, self.native, self.tool], "stdin": self.pipe,
                         "bridgeSha256": next(iter(m.BRIDGES)), "launcherPath": "/fixture-launcher"}
        self.plan = {"version": 1, "processesAbsent": False, "runtime": self.runtime, "agents": [AGENT],
                     "supervisor": self.supervisor, "topology": [self.topology],
                     "baseline": [{"pid": p["pid"], "start": p["start"]} for p in self.live.values()],
                     "histories": [m.history_summary(AGENT)]}
        self.request = {"version": 1, "action": "quiesce", "operationId": OP, "runtime": self.runtime,
                        "agents": [AGENT], "expectedPlan": self.plan}
        self.signals = []
        self.finish = True
        patch.dict(m.__dict__, inventory=lambda: copy.deepcopy(self.live),
                   services=lambda _: [],
                   proc_identity=lambda pid: copy.deepcopy(self.live[pid]),
                   digest_file=lambda path, proc=False: m.LAUNCHER if path == "/fixture-launcher" else self.topology["bridgeSha256"],
                   stdin_owner=lambda *_: self.pipe).start()
        patch.object(m.os, "pidfd_open", self.open_pidfd, create=True).start()
        patch.object(m.signal, "pidfd_send_signal", self.deliver, create=True).start()

    def open_pidfd(self, pid, flags):
        self.assertEqual(pid, 20)
        return os.open(os.devnull, os.O_RDONLY)

    def deliver(self, fd, sig):
        journal = json.loads((self.root / ".cma/recovery" / JOURNAL).read_bytes())
        self.assertEqual(journal["plan"], self.plan, "durable journal must precede every signal")
        self.assertEqual(journal["status"], "admitted")
        self.signals.append(sig)
        self.assertEqual(sig, signal.SIGTERM)
        del self.live[20]
        if self.finish:
            for pid in (30, 40, 50):
                del self.live[pid]
            self.terminal()

    def test_signal_only_bridge_and_preserve_dirty_work_and_history(self):
        dirty = self.root / "dirty.txt"
        dirty.write_bytes(b"uncommitted owner work")
        result = m.quiesce(self.request)
        self.assertEqual(result["status"], "quiesced")
        self.assertEqual(result["histories"][0]["conversationId"], AGENT["conversationId"])
        self.assertEqual(self.signals, [signal.SIGTERM])
        self.assertEqual(dirty.read_bytes(), b"uncommitted owner work")
        self.assertTrue(self.history.read_bytes().startswith(self.initial))
        self.assertEqual(m.quiesce(self.request), result)
        self.assertEqual(len(self.signals), 1, "lost ACK replay must not signal another process")

    def test_lost_connection_after_signal_retries_same_journal(self):
        original = m.write_journal
        def fail_final(parent, name, value):
            if value["status"] == "quiesced":
                raise OSError("lost transport before final journal update")
            return original(parent, name, value)
        with patch.dict(m.__dict__, write_journal=fail_final):
            with self.assertRaises(OSError):
                m.quiesce(self.request)
        self.assertEqual(m.quiesce(self.request)["status"], "quiesced")
        self.assertEqual(len(self.signals), 1)

    def test_bridge_already_dead_uses_exact_inspected_plan(self):
        self.live = {10: self.supervisor}
        self.terminal()
        self.assertEqual(m.quiesce(self.request)["status"], "quiesced")
        self.assertFalse(self.signals)

    def test_pid_reuse_never_signals(self):
        self.live[20] = {**self.bridge, "start": 99999}
        with self.assertRaisesRegex(m.Refused, "process_changed"):
            m.quiesce(self.request)
        self.assertFalse(self.signals)

    def test_changed_parent_or_source_digest_never_signals(self):
        self.live[40] = {**self.native, "ppid": 99}
        with self.assertRaisesRegex(m.Refused, "native_topology_changed"):
            m.quiesce(self.request)
        self.live[40] = self.native
        with patch.dict(m.__dict__, digest_file=lambda *_: "0" * 64):
            with self.assertRaisesRegex(m.Refused, "binary_changed"):
                m.quiesce(self.request)
        self.assertFalse(self.signals)

    def test_tool_child_exec_does_not_retarget_or_block_native_drain(self):
        self.live[50] = {**self.tool, "commandSha256": "a" * 64, "exeSha256": "b" * 64}
        self.assertEqual(m.quiesce(self.request)["status"], "quiesced")
        self.assertEqual(self.signals, [signal.SIGTERM])

    def test_unrequested_process_is_never_exempted_by_old_baseline(self):
        extra = process(99, 1)
        self.live[99] = extra
        self.plan["baseline"].append({"pid": 99, "start": extra["start"]})
        with self.assertRaisesRegex(m.Refused, "unowned_process"):
            m.quiesce(self.request)
        self.assertFalse(self.signals)
        with self.assertRaisesRegex(m.Refused, "untracked_process"):
            m.absent(self.plan, {10: self.supervisor, 99: extra})

    def test_preboot_unmatched_turns_preserved_but_same_id_new_start_refused(self):
        self.plan["histories"][0]["files"][0]["historicalOpenTurns"] = [TURN]
        self.live = {10: self.supervisor}
        result = m.quiesce(self.request)
        self.assertEqual(result["histories"][0]["files"][0]["historicalOpenTurns"], [TURN])
        self.assertEqual(self.history.read_bytes(), self.initial)
        with self.history.open("ab") as file:
            file.write(line("event_msg", {"type": "task_started", "turn_id": TURN}))
        with self.assertRaisesRegex(m.Refused, "native_turn_restarted"):
            m.quiesce(self.request)

    def test_boot_changes_between_pidfd_open_and_signal(self):
        def replaced(pid, flags):
            self.writer["boot_id"] = OP
            self.publish_boot()
            return self.open_pidfd(pid, flags)
        with patch.object(m.os, "pidfd_open", replaced):
            with self.assertRaisesRegex(m.Refused, "boot_changed"):
                m.quiesce(self.request)
        self.assertFalse(self.signals)

    def test_timeout_is_retryable_and_never_escalates(self):
        self.finish = False
        elapsed = [0.0]
        def advance(seconds):
            self.assertEqual(self.signals, [signal.SIGTERM])
            elapsed[0] += seconds
        clock = types.SimpleNamespace(monotonic=lambda: elapsed[0], sleep=advance,
                                      time=time.time)
        # Journal fsync and runner scheduling must not consume the drain budget
        # before this test reaches its deliberately surviving native process.
        with patch.dict(m.__dict__, time=clock, DEADLINE=0.08):
            with self.assertRaisesRegex(m.Refused, "deadline"):
                m.quiesce(self.request)
        self.assertGreaterEqual(elapsed[0], 0.08)
        self.assertEqual(self.signals, [signal.SIGTERM])
        self.live = {10: self.supervisor}
        self.terminal()
        self.assertEqual(m.quiesce(self.request)["status"], "quiesced")
        self.assertEqual(len(self.signals), 1)

    def test_surviving_detached_child_is_not_lost_on_reparenting(self):
        original = self.deliver
        def detached(fd, sig):
            original(fd, sig)
            self.live[99] = process(99, 1)
        with patch.object(m.signal, "pidfd_send_signal", detached):
            with self.assertRaisesRegex(m.Refused, "untracked_process"):
                m.quiesce(self.request)

    def test_exit_without_terminal_history_does_not_claim_success(self):
        self.live = {10: self.supervisor}
        with self.assertRaisesRegex(m.Refused, "native_turn_unsettled"):
            m.quiesce(self.request)

    def preboot_history(self, timestamp="2026-10-28T06:04:26.503Z"):
        self.initial = line("session_meta", {"id": AGENT["conversationId"], "cli_version": "0.147.0"})
        self.initial += line("event_msg", {"type": "task_started", "turn_id": BOOT[:8] + "-3333-7333-8333-333333333333"}, "2026-10-28T04:59:50.666Z")
        self.initial += line("event_msg", {"type": "task_complete", "turn_id": AGENT["conversationId"]}, "2026-10-28T05:59:17.653Z")
        self.initial += line("event_msg", {"type": "task_started", "turn_id": TURN}, timestamp)
        self.history.write_bytes(self.initial)
        self.plan["histories"] = [m.history_summary(AGENT)]
        file = self.plan["histories"][0]["files"][0]
        self.topology.update(nativeStartEpochMs=1793168177880, openHistoryPaths=[file["path"]])
        file["historicalOpenTurns"] = m.historical_turns(file, self.topology)
        self.assertNotIn(TURN, file["historicalOpenTurns"])
        return file

    def test_existing_admitted_journal_accepts_unchanged_preboot_start_after_drain(self):
        self.preboot_history()
        self.terminal = lambda: None
        # Reproduce the previous final-history decision after its graceful EOF.
        with patch.dict(m.__dict__, preboot_turns=lambda *_: []):
            with self.assertRaisesRegex(m.Refused, "native_turn_unsettled"):
                m.quiesce(self.request)
        journal_path = self.root / ".cma/recovery" / JOURNAL
        admitted = json.loads(journal_path.read_bytes())
        self.assertEqual(admitted["status"], "admitted")
        self.assertEqual(set(self.live), {10})
        result = m.quiesce(self.request)
        self.assertEqual(result["status"], "quiesced")
        self.assertIn(TURN, result["histories"][0]["files"][0]["historicalOpenTurns"])
        self.assertEqual(self.history.read_bytes(), self.initial)
        self.assertEqual(result["histories"][0]["conversationId"], AGENT["conversationId"])
        completed = json.loads(journal_path.read_bytes())
        for key in ("operationId", "planSha256", "plan"):
            self.assertEqual(completed[key], admitted[key])
        self.assertEqual(m.quiesce(self.request), result)
        self.assertEqual(self.signals, [signal.SIGTERM])

    def test_preboot_fallback_refuses_live_reused_and_untracked_processes(self):
        self.preboot_history()
        with self.assertRaisesRegex(m.Refused, "process_reappeared"):
            m.final_history(self.plan)
        self.live = {10: self.supervisor, 40: {**self.native, "start": 99999}}
        with self.assertRaisesRegex(m.Refused, "pid_reused"):
            m.final_history(self.plan)
        self.live = {10: self.supervisor, 99: process(99, 1)}
        with self.assertRaisesRegex(m.Refused, "untracked_process"):
            m.final_history(self.plan)

    def test_preboot_fallback_replays_plan_without_legacy_classification_field(self):
        prior = self.preboot_history()
        del prior["historicalOpenTurns"]
        self.live = {10: self.supervisor}
        original_plan = copy.deepcopy(self.plan)
        result = m.quiesce(self.request)
        self.assertEqual(result["histories"][0]["files"][0]["historicalOpenTurns"], prior["openTurns"])
        self.assertEqual(self.plan, original_plan)
        self.assertEqual(self.history.read_bytes(), self.initial)
        self.assertFalse(self.signals)

    def test_preboot_fallback_requires_original_clock_margin(self):
        self.live = {10: self.supervisor}
        for stamp in (None, "invalid", "2026-10-28T06:15:17.880Z", "2026-10-28T06:16:17.880Z", "2026-10-28T06:17:00Z"):
            with self.subTest(timestamp=stamp):
                self.preboot_history(stamp)
                with self.assertRaisesRegex(m.Refused, "native_turn_unsettled"):
                    m.final_history(self.plan)
        self.preboot_history()
        del self.topology["nativeStartEpochMs"]
        with self.assertRaisesRegex(m.Refused, "native_turn_unsettled"):
            m.final_history(self.plan)

    def test_preboot_fallback_does_not_accept_appended_or_restarted_turns(self):
        self.live = {10: self.supervisor}
        for turn, timestamp, error in [
            (AGENT["conversationId"], "2026-10-28T06:04:26.503Z", "native_turn_unsettled"),
            (TURN, "2026-10-28T06:04:26.503Z", "native_turn_restarted"),
            (TURN, "2026-10-28T06:18:00Z", "native_turn_restarted"),
        ]:
            with self.subTest(turn=turn, timestamp=timestamp):
                self.preboot_history()
                with self.history.open("ab") as file:
                    file.write(line("event_msg", {"type": "task_started", "turn_id": turn}, timestamp))
                with self.assertRaisesRegex(m.Refused, error):
                    m.final_history(self.plan)

    def test_preboot_fallback_keeps_exact_prefix_and_start_ordinal(self):
        self.preboot_history()
        self.live = {10: self.supervisor}
        self.history.write_bytes(self.initial.replace(b"0.147.0", b"0.146.0"))
        with self.assertRaisesRegex(m.Refused, "history_prefix_changed"):
            m.final_history(self.plan)
        self.history.write_bytes(self.initial)
        prior = self.plan["histories"][0]["files"][0]
        prior["openTurnStarts"][TURN]["ordinal"] -= 1
        with self.assertRaisesRegex(m.Refused, "native_turn_restarted"):
            m.final_history(self.plan)

    def test_changed_plan_cannot_replay_operation(self):
        m.quiesce(self.request)
        changed = copy.deepcopy(self.request)
        changed["expectedPlan"]["baseline"].append({"pid": 99, "start": 99})
        with self.assertRaisesRegex(m.Refused, "journal_mismatch"):
            m.quiesce(changed)

    def test_journal_symlink_never_overwrites_owner_file(self):
        (self.root / ".cma/recovery").mkdir()
        owner = self.root / "owner.txt"
        owner.write_text("preserve")
        (self.root / ".cma/recovery" / JOURNAL).symlink_to(owner)
        with self.assertRaises(OSError):
            m.quiesce(self.request)
        self.assertEqual(owner.read_text(), "preserve")
        self.assertFalse(self.signals)


class EmptyLiveAgents(Fixture):
    def setUp(self):
        super().setUp()
        self.supervisor = process(1, 0)
        self.live = {1: self.supervisor}
        self.request = {"version": 1, "action": "inspect", "runtime": self.runtime, "agents": []}
        patch.dict(m.__dict__, inventory=lambda: copy.deepcopy(self.live), services=lambda _: [],
                   proc_identity=lambda pid: copy.deepcopy(self.live[pid])).start()
        original_readlink = os.readlink
        def readlink(path, *args, **kwargs):
            if str(path).startswith(m.PROC + "/") and str(path).endswith("/exe"):
                return "/usr/local/bin/layer4-runtime" if str(path) == m.PROC + "/1/exe" else "/unowned/process"
            return original_readlink(path, *args, **kwargs)
        patch.object(m.os, "readlink", readlink).start()
        patch.dict(m.__dict__, send_eof=lambda *_: self.fail("empty admission cannot signal")).start()
        patch.object(m.signal, "pidfd_send_signal", lambda *_: self.fail("no pidfd signal"), create=True).start()
        patch.dict(m.__dict__, history_summary=lambda *_: self.fail("terminal Agent history is not a live conversation")).start()

    def admitted(self):
        m.validate(self.request)
        inspection = m.inspect(self.request)
        request = {**self.request, "action": "quiesce", "operationId": OP, "expectedPlan": inspection["plan"]}
        m.validate(request)
        return request

    def test_empty_inspection_preserves_files_and_current_boot_authority(self):
        before = {p: p.read_bytes() for p in self.root.rglob("*") if p.is_file()}
        request = self.admitted()
        plan = request["expectedPlan"]
        self.assertEqual(plan["runtime"], self.runtime)
        self.assertEqual(plan["supervisor"], self.supervisor)
        self.assertFalse(plan["processesAbsent"], "the verified supervisor runtime is still running")
        for key in ("agents", "topology", "histories"):
            self.assertEqual(plan[key], [])
        self.assertEqual(before, {p: p.read_bytes() for p in self.root.rglob("*") if p.is_file()})
        self.assertFalse((self.root / ".cma/recovery").exists())

    def test_empty_quiescence_journals_without_signal_and_replays_exact_plan(self):
        dirty = self.root / "dirty.txt"
        dirty.write_bytes(b"preserved owner workspace")
        request = self.admitted()
        result = m.quiesce(request)
        self.assertEqual(result["status"], "quiesced")
        self.assertEqual(result["histories"], [])
        journal = json.loads((self.root / ".cma/recovery" / JOURNAL).read_bytes())
        self.assertEqual(journal["plan"], request["expectedPlan"])
        self.assertEqual(journal["status"], "quiesced")
        self.assertEqual(m.quiesce(request), result)
        self.assertEqual(self.history.read_bytes(), self.initial)
        self.assertEqual(dirty.read_bytes(), b"preserved owner workspace")
        changed = copy.deepcopy(request)
        changed["expectedPlan"]["baseline"].append({"pid": 99, "start": 99})
        with self.assertRaisesRegex(m.Refused, "journal_mismatch"):
            m.quiesce(changed)

    def test_empty_inspection_refuses_unrequested_child_or_detached_process(self):
        for parent in (1, 0):
            with self.subTest(parent=parent):
                self.live[99] = process(99, parent)
                with self.assertRaisesRegex(m.Refused, "unowned_process"):
                    m.inspect(self.request)
                del self.live[99]
        self.assertFalse((self.root / ".cma/recovery").exists())

    def test_empty_quiescence_refuses_process_arriving_after_inspection(self):
        request = self.admitted()
        self.live[99] = process(99, 1)
        with self.assertRaisesRegex(m.Refused, "unowned_process"):
            m.quiesce(request)
        self.assertFalse((self.root / ".cma/recovery" / JOURNAL).exists())

    def test_empty_quiescence_refuses_process_arriving_before_final_proof(self):
        request = self.admitted()
        original = m.final_history
        def late(plan):
            self.live[99] = process(99, 0)
            return original(plan)
        with patch.dict(m.__dict__, final_history=late):
            with self.assertRaisesRegex(m.Refused, "untracked_process"):
                m.quiesce(request)
        journal = json.loads((self.root / ".cma/recovery" / JOURNAL).read_bytes())
        self.assertEqual(journal["status"], "admitted")
        del self.live[99]
        self.assertEqual(m.quiesce(request)["status"], "quiesced")

    def test_empty_replay_revalidates_process_absence_and_boot(self):
        request = self.admitted()
        m.quiesce(request)
        self.live[99] = process(99, 0)
        with self.assertRaisesRegex(m.Refused, "untracked_process"):
            m.quiesce(request)
        del self.live[99]
        self.writer["boot_id"] = "f" * 32
        self.publish_boot()
        with self.assertRaisesRegex(m.Refused, "boot_changed"):
            m.quiesce(request)


class Proc(unittest.TestCase):
    def test_plan_digest_matches_adapter_utf8_workspace_paths(self):
        self.assertEqual(m.sha(m.encode({"cwd": "/home/cma/研究", "record": "😀"})),
                         "3bfcf358614050535610e2302cefbc84e5b6284e3df6695151e67d1703b7a0a4")

    def test_only_exact_supervisor_relay_source_and_argv_are_allowed(self):
        source = (SOURCE.parent / "runtime_control_relay.py").read_bytes()
        self.assertEqual(m.sha(source), m.RELAY_SOURCE)
        self.assertIn("e595835b6314ff0770fdf97817d6920295ea384a5ac3bfdd0476b90c81b79a4f", m.RELAY_SOURCES)
        argv = [b"/usr/bin/python3", b"-I", b"-u", b"-c", source]
        self.assertTrue(m.relay_command(b"\0".join(argv) + b"\0"))
        for changed in (argv + [b"extra"], [b"python3", *argv[1:]], [*argv[:-1], source + b"\n"], [*argv[:-1], b"print('unrelated')"]):
            self.assertFalse(m.relay_command(b"\0".join(changed) + b"\0"))

    def test_source_stdio_refuses_missing_extra_shared_or_named_endpoints(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "net").mkdir()
            endpoints = []
            for pid in (20, 30, 40, 50):
                (root / str(pid) / "fd").mkdir(parents=True)
            for fd in range(3):
                (root / "20/fd" / str(21 + fd * 2)).symlink_to("socket:[" + str(100 + fd) + "]")
                for pid in (30, 40):
                    (root / str(pid) / "fd" / str(fd)).symlink_to("socket:[" + str(200 + fd) + "]")
                endpoints.extend([100 + fd, 200 + fd])
            text = "Num RefCount Protocol Flags Type St Inode\n" + "".join(
                "0: 00000003 00000000 00000000 0001 03 " + str(i) + "\n" for i in endpoints)
            (root / "net/unix").write_text(text)
            with patch.dict(m.__dict__, PROC=str(root), DEADLINE=float("inf")):
                result = m.source_stdio(40, 30, 20, [20, 30, 40, 50])
                self.assertEqual(result["mode"], "pinned_source_stdio")
                (root / "20/fd/21").unlink()
                with self.assertRaisesRegex(m.Refused, "stdio_shape"):
                    m.source_stdio(40, 30, 20, [20, 30, 40, 50])
                (root / "20/fd/21").symlink_to("socket:[100]")
                (root / "20/fd/99").symlink_to("socket:[999]")
                with self.assertRaisesRegex(m.Refused, "stdio_shape"):
                    m.source_stdio(40, 30, 20, [20, 30, 40, 50])
                (root / "20/fd/99").unlink()
                (root / "50/fd/7").symlink_to("socket:[100]")
                with self.assertRaisesRegex(m.Refused, "stdio_shared"):
                    m.source_stdio(40, 30, 20, [20, 30, 40, 50])
                (root / "50/fd/7").unlink()
                (root / "net/unix").write_text(text.replace("100\n", "100 /unrelated/socket\n"))
                with self.assertRaisesRegex(m.Refused, "stdio_socket_kind"):
                    m.source_stdio(40, 30, 20, [20, 30, 40, 50])

    def test_exact_pipe_and_socket_peer_owner(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with patch.dict(m.__dict__, PROC=str(root), DEADLINE=float("inf")):
                for pid, fd, target, flags in ((20, 7, "pipe:[123]", "01"), (40, 0, "pipe:[123]", "00")):
                    (root / str(pid) / "fd").mkdir(parents=True)
                    (root / str(pid) / "fdinfo").mkdir()
                    (root / str(pid) / "fd" / str(fd)).symlink_to(target)
                    (root / str(pid) / "fdinfo" / str(fd)).write_text("flags:\t" + flags + "\n")
                self.assertEqual(m.stdin_owner(40, 20, [20, 40])["writer"], "pipe:[123]")
                (root / "40/fdinfo/0").write_text("flags:\t02\n")
                with self.assertRaisesRegex(m.Refused, "stdin_owner_mismatch"):
                    m.stdin_owner(40, 20, [20, 40])
                (root / "20/fd/7").unlink()
                (root / "20/fd/7").symlink_to("socket:[456]")
                (root / "40/fd/0").unlink()
                (root / "40/fd/0").symlink_to("socket:[123]")
                with patch.dict(m.__dict__, unix_peer=lambda inode: 456):
                    self.assertEqual(m.stdin_owner(40, 20, [20, 40])["writer"], "socket:[456]")

    @unittest.skipUnless(os.uname().sysname == "Linux", "Linux UNIX_DIAG and pidfd evidence")
    def test_real_linux_socket_peer_and_pidfd_available(self):
        import socket
        fd = os.pidfd_open(os.getpid(), 0)
        os.close(fd)
        self.assertTrue(hasattr(signal, "pidfd_send_signal"))
        first, second = socket.socketpair()
        with first, second:
            try:
                self.assertEqual(m.unix_peer(os.fstat(first.fileno()).st_ino), os.fstat(second.fileno()).st_ino)
            except m.Refused as error:
                if str(error) != "socket_diagnostic_unsupported":
                    raise
                self.skipTest("kernel UNIX_DIAG absent; exact source stdio fallback tested separately")


if __name__ == "__main__":
    unittest.main()
