"""Recovery admits only the pinned, supervisor-owned desktop process tree."""
import copy
import hashlib
from pathlib import Path
import types
import unittest
from unittest.mock import patch

SOURCE = Path(__file__).parent
m = types.ModuleType("desktop_recovery_fixture")
exec(compile((SOURCE / "recovery_io.py").read_text() + "\n" +
             (SOURCE / "recovery.py").read_text(), "recovery", "exec"), m.__dict__)


def process(pid, parent, session=None):
    return {"pid": pid, "ppid": parent, "sid": session or pid, "start": pid * 100,
            "commandSha256": str(pid), "exeSha256": "fixture"}


class DesktopRecovery(unittest.TestCase):
    def setUp(self):
        self.addCleanup(patch.stopall)
        self.live = {p["pid"]: p for p in [process(1, 0), process(20, 1),
                     process(30, 20), process(40, 30), process(50, 20)]}
        self.command = b"/bin/bash\0/usr/local/bin/cma-desktop-session\0"
        self.guardian = None
        self.environment = {b"HOME": b"/home/cma", b"DISPLAY": b":99"}
        self.source = m.DESKTOP_SOURCE
        self.real_readlink = m.os.readlink
        patch.dict(m.__dict__, read=self.read, environment=lambda _: self.environment,
                   digest_file=lambda _: self.source, inventory=lambda: copy.deepcopy(self.live),
                   proc_identity=lambda pid: copy.deepcopy(self.live[pid])).start()
        patch.object(m.os, "readlink", self.readlink).start()

    def read(self, path, *_):
        if path == "/proc/20/cmdline":
            return self.command
        if self.guardian and path == f"/proc/{self.guardian}/cmdline":
            return m.RUNTIME_PATH.encode() + b"\0" + m.DESKTOP_GUARDIAN + b"\0"
        return b"unrelated\0"

    def readlink(self, path):
        if path == "/proc/20/exe":
            return m.os.path.realpath("/bin/bash")
        if self.guardian and path in ("/proc/1/exe", f"/proc/{self.guardian}/exe"):
            return m.RUNTIME_PATH
        if not str(path).startswith("/proc/"):
            return self.real_readlink(path)
        self.fail("sandboxed browser processes must not require readable exe/environ: " + path)

    def plan(self):
        return {"services": m.services(self.live), "supervisor": self.live[1], "topology": []}

    def enable_supervised_desktop(self):
        self.guardian = 21
        self.live[self.guardian] = process(self.guardian, 1)
        self.live[20].update(ppid=self.guardian, sid=self.guardian)

    def test_installed_launcher_matches_reviewed_source(self):
        local = SOURCE / "../../../layer4-runtime/src/desktop-session.sh"
        source = local if local.is_file() else Path(m.DESKTOP_PATH)
        self.assertEqual(hashlib.sha256(source.read_bytes()).hexdigest(), m.DESKTOP_SOURCE)

    def test_omarchy_launcher_matches_reviewed_source(self):
        source = SOURCE / "../../../layer4-runtime/src/omarchy/session.sh"
        if source.is_file():
            self.assertEqual(hashlib.sha256(source.read_bytes()).hexdigest(), m.OMARCHY_DESKTOP_SOURCE)
        self.source = m.OMARCHY_DESKTOP_SOURCE
        self.assertTrue(m.absent(self.plan(), self.live))

    def test_retained_omarchy_launcher_keeps_recovery_authority(self):
        self.source = "b2b88eaf39c2bf6e6018e2a6185d5461cbaabe1f66fa76562c898c24914af609"
        self.assertTrue(m.absent(self.plan(), self.live))

    def test_retained_xfce_launcher_keeps_recovery_authority(self):
        self.source = "c03e804c28de395c7501001f390346f2c2ad29349eab2e17cf9929e7902bcf99"
        self.assertTrue(m.absent(self.plan(), self.live))

    def test_retained_pre_supervision_launchers_keep_recovery_authority(self):
        for source in (
            "6910bdac6a24cc971f95bc0d2d10f19635a225e6a9f04b333d3555b4cec7ed7e",
            "7b290d8bcf9d95de46c43d069d8b1e73def32f55cfc2ede3e04b7720bc6e439e",
        ):
            with self.subTest(source=source):
                self.source = source
                self.assertTrue(m.absent(self.plan(), self.live))

    def test_children_and_adopted_helpers_are_admitted_by_ancestry(self):
        # A crash helper can have its own session after double-forking. It is
        # still adopted by the launcher subreaper, never the runtime root.
        self.assertEqual({p["pid"] for p in m.services(self.live)}, {20, 30, 40, 50})
        self.assertTrue(m.absent(self.plan(), self.live))

    def test_supervised_guardian_and_its_launcher_are_bound_to_the_recovery_plan(self):
        self.enable_supervised_desktop()
        with patch.object(m, "stdin_owner", return_value={"reader": "pipe:[1]"}) as stdin:
            plan = self.plan()
            self.assertEqual({p["pid"] for p in plan["services"]}, {20, 21, 30, 40, 50})
            self.assertEqual(plan["services"][0]["desktopLauncher"]["pid"], 20)
            stdin.assert_called_once_with(21, 1, (1, 21))
            self.assertTrue(m.absent(plan, self.live))
            self.live[20]["start"] += 1
            with self.assertRaisesRegex(m.Refused, "desktop_changed"):
                m.absent(plan, self.live)

    def test_supervised_guardian_requires_control_pipe_and_exact_launcher(self):
        self.enable_supervised_desktop()
        with patch.object(m, "stdin_owner", side_effect=m.Refused("stdin_owner_mismatch")):
            with self.assertRaisesRegex(m.Refused, "stdin_owner_mismatch"):
                self.plan()
        with patch.object(m, "stdin_owner", return_value={"reader": "pipe:[1]"}):
            self.command = b"unrelated\0"
            with self.assertRaisesRegex(m.Refused, "desktop_identity"):
                self.plan()

    def test_unrelated_sibling_is_not_a_desktop_service(self):
        plan = self.plan()
        self.live[60] = process(60, 1)
        self.assertNotIn(60, {p["pid"] for p in m.services(self.live)})
        with self.assertRaisesRegex(m.Refused, "untracked_process"):
            m.absent(plan, self.live)

    def test_browser_renderer_churn_does_not_replace_the_launcher(self):
        plan = self.plan()
        del self.live[40]
        self.live[45] = process(45, 30)
        self.assertEqual(m.validate_services(plan, self.live), {20, 30, 45, 50})
        self.assertTrue(m.absent(plan, self.live))

    def test_replaced_or_missing_launcher_refuses_prior_plan(self):
        plan = self.plan()
        self.live[20]["start"] += 1
        with self.assertRaisesRegex(m.Refused, "desktop_changed"):
            m.absent(plan, self.live)
        del self.live[20]
        with self.assertRaisesRegex(m.Refused, "desktop_changed"):
            m.absent(plan, self.live)

    def test_retained_openbox_launcher_keeps_recovery_authority(self):
        self.source = "3127c5d454316efaf7e147665f871825aae2eda00cea250d2f89fd18f351263d"
        self.assertEqual({p["pid"] for p in m.services(self.live)}, {20, 30, 40, 50})
        self.assertTrue(m.absent(self.plan(), self.live))

    def test_changed_launcher_source_is_refused(self):
        self.source = "0" * 64
        with self.assertRaisesRegex(m.Refused, "desktop_identity"):
            m.services(self.live)

    def test_agent_launched_script_cannot_masquerade_as_desktop(self):
        self.live[20]["ppid"] = 99
        self.assertEqual(m.services(self.live), [])

    def test_wrong_session_or_command_is_not_admitted(self):
        self.live[20]["sid"] = 1
        self.assertEqual(m.services(self.live), [])
        self.live[20]["sid"] = 20
        self.command = b"/bin/bash\0-c\0/usr/local/bin/cma-desktop-session\0"
        self.assertEqual(m.services(self.live), [])

    def test_agent_home_or_wrong_display_is_refused(self):
        for env in ({**self.environment, b"CODEX_HOME": b"/tmp/agent"},
                    {**self.environment, b"DISPLAY": b":1"}):
            with self.subTest(environment=env):
                self.environment = env
                with self.assertRaisesRegex(m.Refused, "desktop_environment"):
                    m.services(self.live)


if __name__ == "__main__":
    unittest.main()
