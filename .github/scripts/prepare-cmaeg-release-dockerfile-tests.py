#!/usr/bin/env python3
"""Offline gate wiring tests; no compiler, Docker daemon or image is invoked."""
import importlib.util
from pathlib import Path
import shlex
import subprocess
import sys
import tempfile
import unittest


SCRIPT = Path(__file__).with_name("prepare-cmaeg-release-dockerfile.py")
ROOT = SCRIPT.resolve().parents[2]
spec = importlib.util.spec_from_file_location("release", SCRIPT)
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)
DOCKERFILE = (ROOT / "api/crates/cmaeg/Dockerfile").read_text()


class ReleaseRecipeTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.directory = Path(self.temporary.name)

    def test_retains_source_gates_and_complete_media_runtime_stages(self):
        prepared = release.render(DOCKERFILE)
        source = release.stages(DOCKERFILE)
        actual = release.stages(prepared)
        self.assertEqual(actual["media-self-test"], source["media-self-test"])
        self.assertEqual(actual["runtime"], source["runtime"])
        self.assertNotIn("local-preview", actual)
        build = actual["build"]
        self.assertLess(build.index("COPY native/cmaeg /out/cmaeg"), build.index(release.ALLOCATOR_GATE))
        self.assertLess(build.index(release.ALLOCATOR_GATE), build.index(release.SANDBOX_GATE))
        self.assertIn(release.MEDIA_REPORT, actual["runtime"])
        self.assertNotIn("cargo ", build)
        self.assertEqual(prepared.count("COPY native/cmaeg /out/cmaeg"), 1)

    def test_uses_selected_source_runtime_changes(self):
        changed = DOCKERFILE.replace("FROM alpine:3.22 AS runtime", "FROM alpine:3.23 AS runtime")
        changed = changed.replace("EXPOSE 8090", "LABEL local.regression=selected-source\nEXPOSE 8090")
        prepared = release.render(changed)
        self.assertIn("FROM alpine:3.23 AS build", prepared)
        self.assertIn("LABEL local.regression=selected-source", prepared)

    def test_rejects_missing_or_weakened_gates(self):
        mutations = [
            (release.ALLOCATOR_GATE, "RUN file /out/cmaeg"),
            (release.ALLOCATOR_GATE, release.ALLOCATOR_GATE + " || true"),
            (release.SANDBOX_GATE, "RUN true"),
            ('test "${status}" -eq 125', "true"),
            ('test "${status}" -eq 0', "true"),
            ("    status=0", "    status=1"),
            ("RUN --network=none <<'MEDIA_SELF_TEST'", "RUN <<'MEDIA_SELF_TEST'"),
            ("COPY api/crates/cmaeg/tests/fixtures/media/ /opt/cmaeg-media-fixtures/", ""),
            ("USER 10001:10001", "USER root"),
            (release.RUNTIME_BINARY, "COPY native/cmaeg /usr/local/bin/cmaeg"),
            (release.MEDIA_REPORT, "# skipped media stage"),
            ('ENTRYPOINT ["/usr/local/bin/cmaeg"]', 'ENTRYPOINT ["/bin/sh"]'),
        ]
        for old, new in mutations:
            with self.subTest(instruction=old), self.assertRaises(ValueError):
                release.render(DOCKERFILE.replace(old, new))

    def test_rejects_replaced_binary_after_allocator_gate(self):
        changed = DOCKERFILE.replace(release.ALLOCATOR_GATE, release.ALLOCATOR_GATE + "\nCOPY unchecked /out/cmaeg")
        with self.assertRaisesRegex(ValueError, "unexpected instruction"):
            release.render(changed)

    def test_rejects_changed_compiler_route(self):
        for old, new in [
            ("FROM rust:1.96.1-alpine AS build", "FROM scratch AS build"),
            ("cargo build --release", "cargo zigbuild --release"),
            ("cargo build --release", "RUSTC_LINKER=zig cargo build --release"),
            ("WORKDIR /build", "WORKDIR /build\nRUN install-zig"),
        ]:
            with self.subTest(change=new), self.assertRaises(ValueError):
                release.render(DOCKERFILE.replace(old, new))

    def test_rejects_stage_drift(self):
        for changed in [
            DOCKERFILE.replace("AS build\n", "AS renamed-build\n"),
            DOCKERFILE.replace("AS media-self-test\n", "AS renamed-media\n"),
            DOCKERFILE + "\nFROM scratch AS unqualified\n",
            DOCKERFILE + "\nFROM scratch AS runtime\n",
            DOCKERFILE + "\nFROM scratch\n",
            DOCKERFILE.replace("FROM alpine:3.22 AS runtime", "FROM ${UNCHECKED} AS runtime"),
            "",
        ]:
            with self.subTest(source=changed[-100:]), self.assertRaises(ValueError):
                release.render(changed)

    def test_rejects_reordered_or_duplicate_allocator_gate(self):
        for changed in [
            DOCKERFILE.replace(release.ALLOCATOR_GATE, "").replace(
                release.SANDBOX_GATE, release.SANDBOX_GATE + "\n" + release.ALLOCATOR_GATE),
            DOCKERFILE.replace(release.ALLOCATOR_GATE, release.ALLOCATOR_GATE + "\n" + release.ALLOCATOR_GATE),
        ]:
            with self.assertRaises(ValueError):
                release.render(changed)

    def gate_commands(self, binary, next_stage):
        build = release.stages(release.render(DOCKERFILE))["build"]
        allocator = next(line.removeprefix("RUN --network=none ") for line in build.splitlines()
                         if line.startswith("RUN --network=none /out/cmaeg"))
        sandbox = build[build.index("RUN --network=none <<") :].replace("RUN --network=none ", "sh ", 1)
        # Run the generated RUN instructions in order, with Docker's failure
        # semantics. Stub only the ELF, never its qualification control flow.
        commands = f"set -eu\nTARGETARCH=amd64\nexport TARGETARCH\n{allocator}\n{sandbox}\n"
        return commands.replace("/out/cmaeg", shlex.quote(str(binary))) + f"touch {shlex.quote(str(next_stage))}\n"

    def test_success_reaches_sandbox_and_downstream_packaging(self):
        binary = self.directory / "candidate cmaeg"
        calls = self.directory / "calls"
        next_stage = self.directory / "packaged"
        binary.write_text(f'#!/bin/sh\nprintf "%s\\n" "$1" >> {shlex.quote(str(calls))}\n')
        binary.chmod(0o755)
        result = subprocess.run(["sh", "-c", self.gate_commands(binary, next_stage)], capture_output=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(calls.read_text().splitlines(), ["--allocator-self-test", "--media-sandbox-check"])
        self.assertTrue(next_stage.exists())

    def test_allocator_failures_block_sandbox_and_packaging(self):
        binary = self.directory / "cmaeg"
        next_stage = self.directory / "packaged"
        # Retention failure, missing CLI, deadline, and unavailable measurement.
        for status in (1, 64, 124, 125):
            with self.subTest(exit=status):
                calls = self.directory / f"calls-{status}"
                binary.write_text(f'#!/bin/sh\nprintf "%s\\n" "$1" >> {shlex.quote(str(calls))}\nexit {status}\n')
                binary.chmod(0o755)
                result = subprocess.run(["sh", "-c", self.gate_commands(binary, next_stage)], capture_output=True)
                self.assertEqual(result.returncode, status)
                self.assertEqual(calls.read_text().splitlines(), ["--allocator-self-test"])
                self.assertFalse(next_stage.exists())

    def test_missing_executable_cannot_package(self):
        next_stage = self.directory / "packaged"
        result = subprocess.run(["sh", "-c", self.gate_commands(self.directory / "missing", next_stage)], capture_output=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(next_stage.exists())

    def invoke(self, source, output):
        return subprocess.run([sys.executable, "-B", str(SCRIPT), "--source", str(source),
                               "--output", str(output)], capture_output=True, text=True)

    def test_cli_generates_context_from_exact_source(self):
        output = self.directory / "Dockerfile"
        result = self.invoke(ROOT, output)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(output.read_text(), release.render(DOCKERFILE))

    def test_cli_refusal_blocks_callers_packaging(self):
        source = self.directory / "source"
        recipe = source / "api/crates/cmaeg/Dockerfile"
        recipe.parent.mkdir(parents=True)
        recipe.write_text(DOCKERFILE.replace(release.ALLOCATOR_GATE, "RUN true"))
        output = self.directory / "Dockerfile"
        next_stage = self.directory / "image-save"
        command = shlex.join([sys.executable, "-B", str(SCRIPT), "--source", str(source), "--output", str(output)])
        result = subprocess.run(["sh", "-ec", f"{command}\ntouch {shlex.quote(str(next_stage))}"], capture_output=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(output.exists())
        self.assertFalse(next_stage.exists())

    def test_cli_rejects_missing_source_and_invalid_output(self):
        for source, output in [
            (self.directory / "missing", self.directory / "Dockerfile"),
            (ROOT, self.directory / "missing" / "Dockerfile"),
            (ROOT, ROOT / "api/crates/cmaeg/Dockerfile"),
        ]:
            with self.subTest(source=source, output=output):
                result = self.invoke(source, output)
                self.assertEqual(result.returncode, 1)
                self.assertIn("local EG release recipe refused:", result.stderr)


if __name__ == "__main__":
    unittest.main()
