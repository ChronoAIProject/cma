"""Private checkout preparation masks metadata and cleanup stays confined."""
import contextlib
import io
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

import staging_cd_checkout as checkout
from staging_cd_common import Refusal


class CheckoutTests(unittest.TestCase):
    def environment(self, directory):
        return {"GITHUB_WORKSPACE": directory, "GITHUB_RUN_ID": "123",
                "GITHUB_RUN_ATTEMPT": "2", "CD_INFRA_REF": "a" * 40,
                "CD_INFRA_TOKEN": "fixture"}

    def test_prepare_masks_before_checkout_and_requires_immutable_confined_inputs(self):
        with tempfile.TemporaryDirectory() as directory:
            env, masker, metadata = self.environment(directory), Mock(), Mock(return_value="a" * 40)
            self.assertTrue(checkout.prepare_app(env, masker, metadata))
            metadata.assert_called_once_with("fixture", "a" * 40, masker)
            masker.assert_not_called()
            for field, values in {
                "CD_INFRA_REF": ("develop", "bad"),
                "GITHUB_RUN_ID": ("../outside", "0"), "GITHUB_RUN_ATTEMPT": (None, "0"),
                "GITHUB_WORKSPACE": ("relative", directory + "/missing"),
            }.items():
                for value in values:
                    metadata.reset_mock()
                    with self.subTest(field=field, value=value), self.assertRaises(Refusal):
                        checkout.prepare_app(dict(env, **{field: value}), masker, metadata)
                    metadata.assert_not_called()
            with self.assertRaises(Refusal):
                checkout.prepare_app(env, masker, Mock(return_value="b" * 40))
            checkout.checkout_path(env).mkdir(parents=True)
            with self.assertRaises(Refusal):
                checkout.prepare_app(env, masker, metadata)

    def test_cleanup_removes_only_this_run_and_rejects_links(self):
        with tempfile.TemporaryDirectory() as directory:
            env = self.environment(directory)
            path = checkout.checkout_path(env)
            path.mkdir(parents=True)
            sibling = path.with_name("124-1")
            sibling.mkdir()
            (path / "config.json").write_text("fixture")
            self.assertTrue(checkout.cleanup(env, Mock()))
            self.assertFalse(path.exists())
            self.assertTrue(sibling.exists())
            self.assertTrue(checkout.cleanup(env, Mock()))
            path.symlink_to(sibling, target_is_directory=True)
            with self.assertRaises(Refusal):
                checkout.cleanup(env, Mock())
            self.assertTrue(sibling.exists())
            path.unlink()
            path.write_text("fixture")
            with self.assertRaises(Refusal):
                checkout.cleanup(env, Mock())
            path.unlink()
            sibling.rmdir()
            path.parent.rmdir()
            path.parent.symlink_to(Path(directory), target_is_directory=True)
            with self.assertRaises(Refusal):
                checkout.cleanup(env, Mock())

    def test_cli_fails_without_disclosure_and_preserves_deploy_summary(self):
        with tempfile.TemporaryDirectory() as directory:
            for args, expected in ((["helper", "cleanup"], "PASS"), (["helper", "prepare-app"], "FAIL"), (["helper"], "FAIL")):
                env = self.environment(directory)
                env.pop("CD_INFRA_TOKEN")
                summary = Path(directory) / "summary"
                env["GITHUB_STEP_SUMMARY"] = str(summary)
                output = io.StringIO()
                with patch.dict(os.environ, env, clear=True), patch.object(sys, "argv", args), contextlib.redirect_stdout(output):
                    self.assertEqual(checkout.main(), 0 if expected == "PASS" else 1)
                lines = output.getvalue().splitlines()
                if expected == "PASS":
                    self.assertEqual(lines[-1], "PASS private-checkout")
                    self.assertTrue(all(line.startswith("::add-mask::") for line in lines[:-1]))
                    self.assertFalse(summary.exists())
                    continue
                # A failure names its kind and check; the summary gains only that explanation.
                reason = "refused" if args[1:] else "declined"
                self.assertRegex(lines[-2], rf"\AFAIL private-checkout reason={reason}( at=staging_cd_\w+\.py:\d+)? cause=")
                self.assertTrue(lines[-1].startswith("::error "))
                self.assertTrue(all(line.startswith("::add-mask::") for line in lines[:-2]))
                rows = summary.read_text().splitlines()
                summary.unlink()
                self.assertEqual(rows[2:3], [row for row in rows if row.startswith("| private-checkout | ")])
                self.assertEqual(len(rows), 3)


if __name__ == "__main__":
    unittest.main()
