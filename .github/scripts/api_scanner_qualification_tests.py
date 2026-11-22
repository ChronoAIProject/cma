"""Synthetic image extraction proves identity, mandatory cases and owned cleanup."""
import contextlib
import io
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import api_scanner_qualification as qualification

IMAGE = "sha256:" + "a" * 64
SOURCE = "b" * 40
CONTAINER = "c" * 64
BUILD = """/usr/local/bin/gitleaks: go1.26.11
\tpath\tgithub.com/zricethezav/gitleaks/v8
\tdep\tgolang.org/x/crypto\tv0.55.0
\tdep\tgolang.org/x/text\tv0.41.0
\tdep\tgithub.com/klauspost/compress\tv1.18.7
\tbuild\t-tags=gore2regex
\tbuild\tCGO_ENABLED=0
\tbuild\tGOOS=linux
\tbuild\tGOARCH=amd64
"""


class Fixture:
    def __init__(self, mutation=None):
        self.mutation, self.active, self.calls = mutation, False, []

    def command(self, arguments, **kwargs):
        self.calls.append(arguments)
        if arguments[0] == "git":
            return "" if arguments[1] == "status" else SOURCE + "\n"
        if arguments[0] == "bash":
            assert arguments[-1] == "--require-real-binary"
            assert Path(kwargs["env"]["CMA_TEST_GITLEAKS"]).is_absolute()
            assert not self.active
            if self.mutation == "case-error":
                raise subprocess.CalledProcessError(1, arguments)
            cases = qualification.CASES[:]
            if self.mutation == "missing-case": cases.pop()
            if self.mutation == "duplicate-case": cases.append(cases[0])
            return "\n".join("SCANNER_CASE_PASS=" + name for name in cases) + ("\nskip - synthetic" if self.mutation == "skip" else "")
        assert arguments[0] == "docker" and arguments[1] not in {"run", "start", "exec", "pull"}
        if arguments[1:3] == ["image", "inspect"]:
            labels = {"org.opencontainers.image.title": "CMA Agent Manager",
                "org.opencontainers.image.source": "https://github.com/ChronoAIProject/cma",
                "org.opencontainers.image.revision": SOURCE, "org.opencontainers.image.version": "git-" + SOURCE[:12]}
            row = {"Id": IMAGE, "Os": "linux", "Architecture": "amd64", "Config": {"Labels": labels}}
            if self.mutation == "component": labels["org.opencontainers.image.title"] = "CMA Event Gateway"
            if self.mutation == "source": labels["org.opencontainers.image.revision"] = "d" * 40
            if self.mutation == "platform": row["Architecture"] = "arm64"
            if self.mutation == "image": row["Id"] = "sha256:" + "d" * 64
            return json.dumps([row])
        if arguments[1] == "create":
            self.name = arguments[arguments.index("--name") + 1]
            self.label = arguments[arguments.index("--label") + 1]
            self.active = True
            assert arguments[-1] == IMAGE and arguments[arguments.index("--network") + 1] == "none"
            return CONTAINER + "\n"
        if arguments[1] == "inspect":
            key, value = self.label.split("=", 1)
            return json.dumps([{"Image": IMAGE, "Name": "/" + self.name, "Config": {"Labels": {key: value}},
                "State": {"Status": "created", "Running": self.mutation == "started"}}])
        if arguments[1] == "cp":
            if self.mutation == "copy-error": raise subprocess.CalledProcessError(1, arguments)
            path = Path(arguments[-1])
            if self.mutation == "missing-artifact" and path.name == "source.txt": return ""
            if path.name == "gitleaks":
                path.write_bytes(b"\x7fELF\x02\x01" + bytes(12) + b"\x3e\x00" + b"synthetic executable")
                path.chmod(0o700)
                if self.mutation == "not-executable": path.chmod(0o600)
                if self.mutation == "wrong-elf": path.write_bytes(b"synthetic Mach-O")
            elif path.name == "source.txt":
                path.write_text(qualification.SOURCE + ("changed\n" if self.mutation == "provenance-source" else ""))
            elif path.name == "inputs.sha256": path.write_text(qualification.INPUTS)
            elif path.name == "binary.sha256":
                sha = "0" * 64 if self.mutation == "binary-hash" else qualification.digest(path.parent / "gitleaks")
                path.write_text(sha + "  /usr/local/bin/gitleaks\n")
            elif path.name == "buildinfo.txt":
                path.write_text(BUILD.replace("v0.55.0", "v0.54.0") if self.mutation == "modules" else BUILD)
            return ""
        if arguments[1] == "ps": return CONTAINER + "\n" if self.active else ""
        if arguments[1] == "rm":
            assert arguments == ["docker", "rm", "--force", CONTAINER]
            if self.mutation == "cleanup": raise subprocess.CalledProcessError(1, arguments)
            self.active = False
            return CONTAINER
        raise AssertionError("unexpected fixture command")


class QualificationTests(unittest.TestCase):
    def test_exact_image_runs_all_cases_after_owned_container_cleanup(self):
        fixture = Fixture()
        with tempfile.TemporaryDirectory() as temporary, patch.object(qualification, "command", fixture.command), \
             contextlib.redirect_stdout(io.StringIO()):
            output = Path(temporary) / "receipt.json"
            self.assertEqual(qualification.qualify(IMAGE, SOURCE, output), 0)
            receipt = json.loads(output.read_text())
        self.assertTrue(receipt["passed"] and receipt["container_cleanup"] and receipt["temporary_cleanup"])
        self.assertEqual(receipt["cases"], qualification.CASES)
        self.assertEqual(receipt["image"], {"id": IMAGE, "source": SOURCE, "platform": "linux/amd64"})
        self.assertEqual(receipt["attempt"], 1)
        self.assertEqual(len([call for call in fixture.calls if call[0] == "bash"]), 1)

    def test_identity_provenance_cases_and_cleanup_fail_closed(self):
        for mutation in ("component", "source", "platform", "image", "started", "copy-error", "missing-artifact",
                "not-executable", "wrong-elf", "provenance-source", "binary-hash", "modules", "missing-case",
                "duplicate-case", "skip", "case-error", "cleanup"):
            with self.subTest(mutation=mutation), tempfile.TemporaryDirectory() as temporary:
                fixture, output = Fixture(mutation), Path(temporary) / "receipt.json"
                with patch.object(qualification, "command", fixture.command), contextlib.redirect_stdout(io.StringIO()):
                    self.assertEqual(qualification.qualify(IMAGE, SOURCE, output), 1)
                receipt = json.loads(output.read_text())
                self.assertFalse(receipt["passed"])
                self.assertIn("failure", receipt)
                if mutation != "cleanup": self.assertFalse(fixture.active)
                if "temporary_cleanup" in receipt: self.assertTrue(receipt["temporary_cleanup"])

    def test_mutable_image_tags_are_refused_before_docker(self):
        with patch.object(qualification, "command") as command:
            with self.assertRaises(ValueError): qualification.inspect_image("synthetic:ci", SOURCE)
            command.assert_not_called()

    def test_provenance_pins_match_the_checked_recipe(self):
        recipe = (qualification.ROOT / "api/crates/cma/Dockerfile").read_text()
        for line in qualification.SOURCE.splitlines():
            self.assertIn(line, recipe)
        for line in qualification.INPUTS.splitlines():
            self.assertIn(line, recipe)
        for fragment in ("golang.org/x/crypto@v0.55.0", "golang.org/x/text@v0.41.0",
                         "github.com/klauspost/compress@v1.18.7", "-tags gore2regex"):
            self.assertIn(fragment, recipe)


if __name__ == "__main__":
    unittest.main()
