import copy
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from build_patch import ACTIONS, HERE, RECORDS, render, validate

REPOSITORY = HERE.parents[3]
REVISION = "1630d06798cbb148d11cad68f9fc43b1f05f280c"
CONFIG = {
    "schema": "eg_records_fault_v1",
    "database_name": "eg_diag_10000000000040008000000000000001",
    "admission_root": "20000000-0000-4000-8000-000000000002",
    "case_id": "30000000-0000-4000-8000-000000000003",
    "action": "pre_submit_refusal",
}


class PatchTest(unittest.TestCase):
    def test_all_actions_apply_exactly_and_preserve_retry_source(self):
        for action in ACTIONS:
            with (
                self.subTest(action=action),
                tempfile.TemporaryDirectory() as directory,
            ):
                patch, patched, binding = render(
                    REPOSITORY, REVISION, {**CONFIG, "action": action}
                )
                root = Path(directory)
                source_path = RECORDS + "mongo_replicas.rs"
                original = subprocess.check_output(
                    ["git", "show", REVISION + ":" + source_path], cwd=REPOSITORY
                )
                target = root / source_path
                target.parent.mkdir(parents=True)
                target.write_bytes(original)
                subprocess.run(
                    ["git", "apply", "--check", "-"],
                    input=patch.encode(),
                    cwd=root,
                    check=True,
                )
                subprocess.run(
                    ["git", "apply", "-"], input=patch.encode(), cwd=root, check=True
                )
                for path, value in patched.items():
                    self.assertEqual((root / path).read_text(), value)
                self.assertNotIn(RECORDS + "mongo_replica_retry.rs", patched)
                self.assertFalse(binding["runtime_or_execution_authorization"])
                self.assertNotIn("std::env", patch)
                self.assertEqual(
                    patched[source_path].count("fault.before_submission"), 1
                )
                self.assertEqual(patched[source_path].count("fault.after_commit"), 1)
                self.assertLess(
                    patched[source_path].index("fault.before_submission"),
                    patched[source_path].index(".start_session()"),
                )
                self.assertGreater(
                    patched[source_path].index("fault.after_commit"),
                    patched[source_path].index(".commit_transaction()"),
                )

    def test_configuration_requires_exact_generated_fixture_identity(self):
        validate(CONFIG)
        mutations = [
            {"database_name": "production"},
            {"database_name": "eg_diag_" + "a" * 32},
            {"admission_root": "not-a-uuid"},
            {"case_id": "30000000-0000-1000-8000-000000000003"},
            {"action": "known_abort"},
            {"action": "healthy"},
            {"extra": "disallowed"},
            {"schema": "unknown"},
            {"case_id": None},
        ]
        for mutation in mutations:
            with (
                self.subTest(mutation=mutation),
                self.assertRaises((ValueError, TypeError)),
            ):
                validate({**CONFIG, **mutation})

    def test_patch_is_deterministic_and_config_changes_are_bound(self):
        left = render(REPOSITORY, REVISION, CONFIG)
        self.assertEqual(left, render(REPOSITORY, REVISION, copy.deepcopy(CONFIG)))
        other = render(
            REPOSITORY,
            REVISION,
            {**CONFIG, "case_id": "40000000-0000-4000-8000-000000000004"},
        )
        self.assertNotEqual(left[2]["patch_sha256"], other[2]["patch_sha256"])
        self.assertNotEqual(
            left[2]["selectors"]["case_sha256"], other[2]["selectors"]["case_sha256"]
        )
        self.assertEqual(
            left[2]["selectors"]["admission_sha256"],
            other[2]["selectors"]["admission_sha256"],
        )

    def test_unreviewed_source_and_mutable_revision_are_rejected(self):
        for revision in [
            "HEAD",
            "aadd212c2810fb12d443998e3cf75bab2fd670cf",
            "70edad1e3f01069841361a90ba40dfd23b34a058",
        ]:
            with self.subTest(revision=revision), self.assertRaises(ValueError):
                render(REPOSITORY, revision, CONFIG)

    def test_cli_only_writes_a_new_bound_packet_and_refuses_overwrite(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            config = root / "config.json"
            config.write_text(json.dumps(CONFIG))
            output = root / "packet"
            command = [
                sys.executable,
                str(HERE / "build_patch.py"),
                "--repository",
                str(REPOSITORY),
                "--revision",
                REVISION,
                "--config",
                str(config),
                "--output",
                str(output),
            ]
            result = subprocess.run(command, capture_output=True, check=True, text=True)
            patch, sources, binding = render(REPOSITORY, REVISION, CONFIG)
            self.assertEqual(
                json.loads(result.stdout)["patch_sha256"], binding["patch_sha256"]
            )
            self.assertEqual((output / "diagnostic.patch").read_text(), patch)
            self.assertEqual(json.loads((output / "binding.json").read_text()), binding)
            for path, value in sources.items():
                self.assertEqual((output / "source" / path).read_text(), value)
            snapshot = {
                str(path.relative_to(output)): path.read_bytes()
                for path in output.rglob("*")
                if path.is_file()
            }
            retry = subprocess.run(command, capture_output=True)
            self.assertNotEqual(retry.returncode, 0)
            self.assertEqual(
                snapshot,
                {
                    str(path.relative_to(output)): path.read_bytes()
                    for path in output.rglob("*")
                    if path.is_file()
                },
            )


if __name__ == "__main__":
    unittest.main()
