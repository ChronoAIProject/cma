import io
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path

from os_refresh_key import SnapshotPinError, main, pinned_snapshot, refresh_key

PINNED = "# syntax=docker/dockerfile:1\nARG DEBIAN_SNAPSHOT=20261121T100000Z\nFROM debian:trixie AS base\nARG DEBIAN_SNAPSHOT\n"


class OsRefreshKeyTests(unittest.TestCase):
    def test_runtime_components_key_on_the_single_top_level_pin(self):
        for component in ("runtime", "runtime-omarchy"):
            self.assertEqual(refresh_key(component, PINNED, "run-1"), "20261121T100000Z")

    def test_bare_stage_redeclarations_are_not_pins(self):
        self.assertEqual(pinned_snapshot(PINNED + "FROM base AS later\nARG DEBIAN_SNAPSHOT\n"), "20261121T100000Z")

    def test_missing_malformed_or_duplicate_pins_are_refused(self):
        for text in ("FROM debian:trixie\n", "ARG DEBIAN_SNAPSHOT=latest\n", "ARG DEBIAN_SNAPSHOT=20261121\n",
                     "ARG DEBIAN_SNAPSHOT=20261121T100000Z \n", " ARG DEBIAN_SNAPSHOT = 20261121T100000Z\n",
                     PINNED + "ARG DEBIAN_SNAPSHOT=20261128T100000Z\n"):
            with self.subTest(text=text), self.assertRaisesRegex(SnapshotPinError, "exactly one"):
                refresh_key("runtime", text, "run-1")

    def test_other_images_keep_their_per_build_key(self):
        for component in ("web", "am", "eg", "api", "cmaeg"):
            self.assertEqual(refresh_key(component, "", "1234-1"), "1234-1")
        with self.assertRaises(SnapshotPinError):
            refresh_key("web", "", "")

    def test_cli_prints_a_github_output_line_or_an_annotated_error(self):
        with tempfile.TemporaryDirectory() as directory:
            dockerfile = Path(directory) / "Dockerfile"
            dockerfile.write_text(PINNED)
            out = io.StringIO()
            with redirect_stdout(out):
                self.assertEqual(main(["x", "runtime", str(dockerfile), "9-1"]), 0)
            self.assertEqual(out.getvalue(), "value=20261121T100000Z\n")
            dockerfile.write_text("FROM debian:trixie\n")
            err = io.StringIO()
            with redirect_stderr(err), redirect_stdout(io.StringIO()):
                self.assertEqual(main(["x", "runtime-omarchy", str(dockerfile), "9-1"]), 1)
            self.assertIn("::error title=OS refresh key unavailable::", err.getvalue())


if __name__ == "__main__":
    unittest.main()
