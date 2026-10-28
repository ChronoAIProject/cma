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


def line(kind, payload):
    return json.dumps({"type": kind, "payload": payload}).encode() + b"\n"


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
        for changes in ({"operationId": OP}, {"agents": []}, {"agents": [AGENT] * 17}, {"action": "kill"}):
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
        with patch.dict(m.__dict__, DEADLINE=time.monotonic() + 0.08):
            with self.assertRaisesRegex(m.Refused, "deadline"):
                m.quiesce(self.request)
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


class Proc(unittest.TestCase):
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
        first, second = socket.socketpair()
        with first, second:
            self.assertEqual(m.unix_peer(os.fstat(first.fileno()).st_ino), os.fstat(second.fileno()).st_ino)
        fd = os.pidfd_open(os.getpid(), 0)
        os.close(fd)
        self.assertTrue(hasattr(signal, "pidfd_send_signal"))


if __name__ == "__main__":
    unittest.main()
