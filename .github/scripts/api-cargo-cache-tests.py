#!/usr/bin/env python3
"""Fast trust-boundary, invalidation and archive regressions; no Docker needed."""
import copy
import contextlib
import datetime
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch

from api_cargo_cache_archive_cases import CacheArchiveCases


SPEC = importlib.util.spec_from_file_location("api_cargo_cache", Path(__file__).with_name("api-cargo-cache.py"))
cache = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(cache)


class CacheTests(CacheArchiveCases, unittest.TestCase):
    cache = cache

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.prefix = "test-compatible-inputs-"
        self.env = {"GITHUB_REPOSITORY": cache.REPOSITORY, "GITHUB_SHA": "a" * 40,
                    "GITHUB_EVENT_NAME": "pull_request", "GITHUB_REF": "refs/pull/12/merge"}
        self.event = {"repository": {"full_name": cache.REPOSITORY}, "number": 12,
                      "pull_request": {"head": {"repo": {"full_name": cache.REPOSITORY}},
                                       "base": {"repo": {"full_name": cache.REPOSITORY}, "ref": "develop"}}}
        # Restores publish the build namespace as a step output.
        outputs = tempfile.TemporaryDirectory()
        self.addCleanup(outputs.cleanup)
        self.output = Path(outputs.name) / "github-output"
        self.output.touch()
        environment = patch.dict(os.environ, {"GITHUB_OUTPUT": str(self.output)})
        environment.start()
        self.addCleanup(environment.stop)

    def bundle(self, member=None, link=None):
        self.root.mkdir(exist_ok=True)
        for name in cache.MOUNTS:
            with tarfile.open(self.root / f"{name}.tar.gz", "w:gz") as archive:
                info = tarfile.TarInfo(member or "./release/example")
                content = b"cached compiled object"
                if link:
                    info.type, info.linkname = link
                    if info.islnk():
                        target = tarfile.TarInfo(info.linkname)
                        target.size = len(content)
                        archive.addfile(target, io.BytesIO(content))
                else:
                    info.size = len(content)
                archive.addfile(info, io.BytesIO(content) if not link else None)
        self.manifest()

    def manifest(self):
        value = {"format": cache.FORMAT, "prefix": self.prefix,
                 "archives": {name: cache.digest(self.root / f"{name}.tar.gz") for name in cache.MOUNTS}}
        (self.root / "manifest.json").write_text(json.dumps(value))

    def inputs(self):
        for name in ("api/crates/cma/Dockerfile", "api/Cargo.lock", "api/Cargo.toml", "api/rust-toolchain.toml",
                     "api/crates/cma/Cargo.toml", ".github/scripts/api-cargo-cache.py"):
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(name)
        return datetime.datetime(2026, 10, 14, tzinfo=datetime.timezone.utc)

    def test_only_same_repository_merge_refs_and_develop_pushes_are_accepted(self):
        cache.authority(self.env, self.event)
        env = dict(self.env, GITHUB_EVENT_NAME="push", GITHUB_REF="refs/heads/develop")
        cache.authority(env, self.event)
        for field, changed in (("GITHUB_REPOSITORY", "other/project"), ("GITHUB_SHA", "short"),
                               ("GITHUB_EVENT_NAME", "pull_request_target"), ("GITHUB_REF", "refs/heads/develop")):
            with self.subTest(field=field), self.assertRaises(ValueError):
                cache.authority(dict(self.env, **{field: changed}), self.event)
        with self.assertRaises(ValueError):
            cache.authority(dict(env, GITHUB_REF="refs/heads/main"), self.event)
        for ref in ("refs/heads/tl/release/train-w2", "refs/heads/tl/release/nested/train"):
            with self.subTest(ref=ref), self.assertRaisesRegex(ValueError, "only develop push"):
                cache.authority(dict(env, GITHUB_REF=ref), self.event)

    def test_develop_merge_queue_entries_may_restore(self):
        ref = "refs/heads/gh-readonly-queue/develop/pr-12-" + "b" * 40
        env = dict(self.env, GITHUB_EVENT_NAME="merge_group", GITHUB_REF=ref)
        event = {"repository": {"full_name": cache.REPOSITORY},
                 "merge_group": {"base_ref": "refs/heads/develop", "head_ref": ref, "head_sha": "a" * 40}}
        cache.authority(env, event)
        main = "refs/heads/gh-readonly-queue/main/pr-12-" + "b" * 40
        for changes, group in ((dict(GITHUB_REF=main), dict(head_ref=main)), ({}, dict(base_ref="refs/heads/main")),
                               ({}, dict(head_ref=ref.replace("pr-12", "pr-13"))), ({}, dict(head_sha="c" * 40)),
                               (dict(GITHUB_REF="refs/heads/develop"), dict(head_ref="refs/heads/develop")),
                               (dict(GITHUB_REPOSITORY="other/project"), {})):
            candidate = dict(event, merge_group=dict(event["merge_group"], **group))
            with self.subTest(changes=changes, group=group), self.assertRaises(ValueError):
                cache.authority(dict(env, **changes), candidate)
        with self.assertRaises(ValueError):
            cache.authority(env, dict(event, repository={"full_name": "fork/project"}))

    def test_fork_and_mismatched_event_authority_are_rejected(self):
        for route in (("repository", "full_name"), ("pull_request", "head", "repo", "full_name"),
                      ("pull_request", "base", "repo", "full_name"), ("pull_request", "base", "ref"), ("number",)):
            event = copy.deepcopy(self.event)
            target = event
            for name in route[:-1]:
                target = target[name]
            target[route[-1]] = "different"
            with self.subTest(route=route), self.assertRaises(ValueError):
                cache.authority(self.env, event)

    def test_source_changes_reuse_dependencies_but_compatibility_changes_do_not(self):
        now = self.inputs()
        prefix = cache.compatibility(self.root, now)
        source = self.root / "api/crates/cma/main.rs"
        source.write_text("changed source")
        self.assertEqual(prefix, cache.compatibility(self.root, now))
        # Build artifacts must never accidentally become cache key inputs.
        generated = self.root / "api/target/fixture/Cargo.toml"
        generated.parent.mkdir(parents=True)
        generated.write_text("generated")
        self.assertEqual(prefix, cache.compatibility(self.root, now))
        for name in ("api/Cargo.lock", "api/Cargo.toml", "api/crates/cma/Cargo.toml", "api/rust-toolchain.toml",
                     "api/crates/cma/Dockerfile", ".github/scripts/api-cargo-cache.py"):
            path = self.root / name
            original = path.read_text()
            path.write_text(original + " changed")
            with self.subTest(name=name):
                self.assertNotEqual(prefix, cache.compatibility(self.root, now))
            path.write_text(original)
        configuration = self.root / "api/.cargo/config.toml"
        configuration.parent.mkdir()
        configuration.write_text("[build]\nrustflags = []\n")
        self.assertNotEqual(prefix, cache.compatibility(self.root, now))

    def test_compatibility_refreshes_weekly_and_stays_bounded(self):
        now = self.inputs()
        prefix = cache.compatibility(self.root, now)
        self.assertEqual(prefix, cache.compatibility(self.root, now + datetime.timedelta(days=1)))
        self.assertNotEqual(prefix, cache.compatibility(self.root, now + datetime.timedelta(days=7)))
        self.assertLess(len(prefix) + 40, 512)

    def test_ci_bounds_match_the_verified_staging_bundle(self):
        # A 1.35 GB develop bundle exceeded the old 1 GiB bound, so CI never saved one.
        import sys
        sys.path.insert(0, str(Path(__file__).parent))
        import staging_cd_cargo_artifact as staging
        self.assertEqual(cache.MAX_ARCHIVE_BYTES, staging.MAX_BUNDLE_BYTES)
        self.assertEqual(cache.MAX_CONTENT_BYTES, staging.MAX_EXPANDED_BYTES)
        self.assertGreater(cache.MAX_ARCHIVE_BYTES, 1_350_000_000)

    def test_one_cache_key_per_compatible_prefix_and_utc_day(self):
        # A per-commit key would store one 1-2 GB entry per develop push.
        def prepared(sha, now):
            with tempfile.TemporaryDirectory() as directory, \
                    patch.dict(os.environ, {"GITHUB_SHA": sha, "GITHUB_OUTPUT": os.devnull}), \
                    patch.object(cache, "compatibility", return_value="cma-api-cargo-v1-prefix-"), \
                    patch.object(cache.datetime, "datetime", wraps=datetime.datetime) as clock:
                clock.now.return_value = now
                target = Path(directory) / "state"
                cache.prepare(target)
                return json.loads((target / "state.json").read_text())
        day = datetime.datetime(2026, 11, 24, 3, 0, tzinfo=datetime.timezone.utc)
        first = prepared("a" * 40, day)
        self.assertEqual(first, {"prefix": "cma-api-cargo-v1-prefix-", "key": "cma-api-cargo-v1-prefix-20261124"})
        self.assertEqual(prepared("b" * 40, day.replace(hour=23)), first)
        self.assertEqual(prepared("a" * 40, day + datetime.timedelta(days=1))["key"],
                         "cma-api-cargo-v1-prefix-20261125")
        self.assertTrue(first["key"].startswith(first["prefix"]))

    def test_a_cache_miss_never_invokes_docker_or_skips_the_build(self):
        (self.root / "state.json").write_text(json.dumps({"prefix": self.prefix}))
        with patch.object(cache, "transfer") as transfer, contextlib.redirect_stdout(io.StringIO()):
            cache.run("inject", self.root)
        transfer.assert_not_called()

    def test_malformed_archive_metadata_falls_back_without_invoking_docker(self):
        directory = self.root
        self.root = directory / "data"
        self.bundle()
        (directory / "state.json").write_text(json.dumps({"prefix": self.prefix}))
        for archives in (None, 42, list(cache.MOUNTS), {"unexpected": "digest"}):
            self.manifest()
            path = self.root / "manifest.json"
            manifest = json.loads(path.read_text())
            manifest["archives"] = archives
            path.write_text(json.dumps(manifest))
            # The worker validates the bundle; a refusal stops it before any transfer.
            with self.subTest(archives=archives), \
                    patch.object(cache.sys, "argv", ["cache", "inject-worker"]), \
                    patch.object(cache, "context", return_value=directory), \
                    patch.object(cache, "transfer") as transfer, \
                    contextlib.redirect_stdout(io.StringIO()) as output, self.assertRaises(SystemExit) as stopped:
                cache.main()
            self.assertEqual(stopped.exception.code, 1)
            transfer.assert_not_called()
            self.assertIn("API Cargo cache worker stopped (ValueError)", output.getvalue())

    def test_graph_fallback_excludes_unpackaged_and_unverified_registry_contents(self):
        # Either file might exist in the previous graph's registry archive.
        # The partial importer never supplies that archive to the builder.
        self.bundle()
        registry = self.root / "registry.tar.gz"
        with tarfile.open(registry, "w:gz") as tar:
            for path in ("src/test-index/demo-1.0.0/lib.rs", "cache/test-index/demo-1.0.0.crate"):
                info = tarfile.TarInfo(path)
                value = b"not the current lockfile content"
                info.size = len(value)
                tar.addfile(info, io.BytesIO(value))
        recipe = cache.dockerfile("inject-target")
        self.assertIn("--mount=type=bind,source=target.tar.gz,target=/restore/target.tar.gz", recipe)
        self.assertNotIn("COPY", recipe)
        self.assertNotIn("registry", recipe)
        with patch("subprocess.run") as run:
            cache.transfer("inject-target", self.root, namespace="synthetic")
        self.assertIn("BUILDKIT_CACHE_MOUNT_NS=synthetic", run.call_args.args[0])
        self.assertEqual(run.call_args.kwargs["input"], recipe)

    def test_full_restore_binds_each_archive_and_extracts_into_locked_cache_mounts(self):
        recipe = cache.dockerfile("inject")
        for name in cache.MOUNTS:
            with self.subTest(mount=name):
                self.assertIn(f"--mount=type=bind,source={name}.tar.gz,target=/restore/{name}.tar.gz", recipe)
                self.assertIn(f"target=/cache/{name},sharing=locked", recipe)
                self.assertIn(f"tar -xzpf /restore/{name}.tar.gz -C /cache/{name}", recipe)
        self.assertIn("RUN --network=none", recipe)
        self.assertNotIn("COPY", recipe)
        self.assertNotIn(",rw", recipe)

    def test_bridge_mount_ids_match_the_real_api_dockerfile(self):
        definition = (cache.ROOT / "api/crates/cma/Dockerfile").read_text()
        self.assertIn(f'id={cache.MOUNTS["registry"]},target=/usr/local/cargo/registry,sharing=locked', definition)
        self.assertIn('id=cma-api-target-${TARGETARCH},target=/build/target,sharing=locked', definition)
        self.assertEqual(cache.MOUNTS["target"], "cma-api-target-amd64")

    def test_cache_trim_keeps_the_newest_completed_session_of_every_crate(self):
        # Each recipe cleans every workspace crate before building, so a crate
        # whose incremental state the trim drops is compiled from scratch.
        crates = ("cma-0", "cmaeg-0", "layer1_public-0", "layer2_core-0", "layer3_adapter-0",
                  "cma_service_auth-0", "build_script_build-0")
        for recipe in ("api/crates/cma/Dockerfile", "api/crates/cmaeg/Dockerfile"):
            with self.subTest(recipe=recipe):
                definition = (cache.ROOT / recipe).read_text()
                script = definition.split("<<'CACHE_TRIM'\n", 1)[1].split("\nCACHE_TRIM\n", 1)[0]
                work = self.root / Path(recipe).parent.name
                release = work / "target/release"
                for name in ("cma", "deps/libdependency.rlib", ".fingerprint/dependency/lib",
                             "build/layer3-adapter-0/out/runtime_versions.rs"):
                    (release / name).parent.mkdir(parents=True, exist_ok=True)
                    (release / name).write_bytes(b"final or dependency output")
                (work / "target/.rustc_info.json").write_text("{}")
                incremental = release / "incremental"
                for crate in crates:
                    # Glob order differs from age, so only modification times select.
                    for stamp, session in ((1000, "s-z-preceding"), (2000, "s-a-newest")):
                        products = incremental / crate / session / "work-products.bin"
                        products.parent.mkdir(parents=True)
                        products.write_bytes(b"compiler session")
                        os.utime(products, (stamp, stamp))
                    (incremental / crate / "s-a-newest.lock").write_bytes(b"")
                    (incremental / crate / "s-m-interrupted-working").mkdir()
                subprocess.run(["sh", "-c", script], cwd=work, check=True)
                self.assertEqual([path.name for path in release.iterdir()], ["incremental"])
                self.assertEqual(sorted(path.name for path in incremental.iterdir()), sorted(crates))
                for crate in crates:
                    self.assertEqual(sorted(path.name for path in (incremental / crate).iterdir()),
                                     ["s-a-newest", "s-a-newest.lock"], crate)
                    self.assertTrue((incremental / crate / "s-a-newest/work-products.bin").is_file(), crate)
                self.assertTrue((work / "target/.rustc_info.json").is_file())

    def test_transfer_always_executes_and_uses_only_expected_mount_ids(self):
        nonces = []
        for operation in ("inject", "extract"):
            with self.subTest(operation=operation), patch.object(cache.subprocess, "run") as run:
                cache.transfer(operation, self.root, "test-builder")
                command = run.call_args.args[0]
                self.assertNotIn("--no-cache", command)
                nonces.append(command[command.index("--build-arg") + 1])
                self.assertEqual(command[command.index("--builder") + 1], "test-builder")
                self.assertEqual(run.call_args.kwargs["timeout"], 150)
                definition = run.call_args.kwargs["input"]
                self.assertIn('test -n "$CMA_CACHE_TRANSFER"', definition)
                self.assertIn("--network=none", definition)
                for name, mount in cache.MOUNTS.items():
                    self.assertIn(f"id={mount},target=/cache/{name},sharing=locked", definition)
                self.assertNotIn("--push", command)
        self.assertNotEqual(nonces[0], nonces[1])

    def namespaces(self):
        return [line.split("=", 1)[1] for line in self.output.read_text().splitlines() if line.startswith("namespace=")]

    def restorable(self):
        directory = self.root / "work"
        directory.mkdir()
        (directory / "state.json").write_text(json.dumps({"prefix": self.prefix}))
        (directory / "data").mkdir()
        return directory

    def test_builds_use_the_restored_namespace_only_after_extraction_succeeds(self):
        directory = self.restorable()
        with patch.object(cache, "validate_bundle"), patch.object(cache, "transfer") as transfer, \
                contextlib.redirect_stdout(io.StringIO()):
            cache.run("inject", directory)
        candidate = transfer.call_args.kwargs["namespace"]
        self.assertRegex(candidate, r"^[0-9a-f]{32}$")
        self.assertEqual(self.namespaces(), [candidate])
        self.assertEqual(json.loads((directory / "state.json").read_text())["build_namespace"], candidate)

    def test_an_interrupted_restore_leaves_the_builds_on_a_clean_namespace(self):
        # SA-07: a transfer that stops after writing part of the mounts must
        # never become the namespace the image builds use.
        directory = self.restorable()
        with patch.object(cache, "validate_bundle"), \
                patch.object(cache, "transfer", side_effect=subprocess.TimeoutExpired("docker", 150)) as transfer, \
                self.assertRaises(subprocess.TimeoutExpired):
            cache.run("inject", directory)
        written = transfer.call_args.kwargs["namespace"]
        self.assertNotIn("build_namespace", json.loads((directory / "state.json").read_text()))
        self.assertEqual(self.namespaces(), [])
        with patch.object(cache.sys, "argv", ["cache", "inject"]), patch.object(cache, "context", return_value=directory), \
                patch.object(cache, "bounded", side_effect=ValueError("cache worker failed")), \
                contextlib.redirect_stdout(io.StringIO()):
            cache.main()
        [selected] = self.namespaces()
        self.assertNotEqual(selected, written)
        self.assertEqual(json.loads((directory / "state.json").read_text())["build_namespace"], selected)

    def test_a_blocked_worker_is_killed_inside_the_step_bound(self):
        # SA-07: validation, transfer or sealing that never returns must not
        # outlive the step; restore falls back to a clean namespace, export saves nothing.
        import time as clock
        for operation in ("inject", "extract"):
            with self.subTest(operation=operation):
                directory = self.restorable() if operation == "inject" else self.root / "export"
                directory.mkdir(exist_ok=True)
                (directory / "state.json").write_text(json.dumps({"prefix": self.prefix}))
                self.output.write_text("")
                pid_file = directory / "grandchild.pid"
                blocked = [sys.executable, "-c",
                           "import subprocess, sys, time; child = subprocess.Popen(['sleep', '60']); "
                           f"open({str(pid_file)!r}, 'w').write(str(child.pid)); time.sleep(60)"]
                started = clock.monotonic()
                with patch.object(cache.sys, "argv", ["cache", operation]), patch.object(cache, "context", return_value=directory), \
                        patch.object(cache, "worker_command", return_value=blocked), \
                        patch.dict(cache.WORKER_SECONDS, {operation: 1.5}), contextlib.redirect_stdout(io.StringIO()):
                    cache.main()
                self.assertLess(clock.monotonic() - started, 10)
                lines = self.output.read_text().splitlines()
                if operation == "inject":
                    [selected] = self.namespaces()
                    self.assertEqual(json.loads((directory / "state.json").read_text())["build_namespace"], selected)
                else:
                    self.assertEqual(lines, ["ready=false"])
                # The worker's whole process group is gone, including its children.
                for _ in range(50):
                    try:
                        os.kill(int(pid_file.read_text()), 0)
                    except ProcessLookupError:
                        break
                    clock.sleep(0.1)
                else:
                    self.fail("worker child outlived the bound")

    def test_only_expected_worker_outputs_reach_the_step(self):
        directory = self.restorable()
        script = ("import os; open(os.environ['GITHUB_OUTPUT'], 'a').write("
                  "'namespace=' + 'd' * 32 + '\\nready=true\\nother=1\\n')")
        with patch.object(cache, "worker_command", return_value=[sys.executable, "-c", script]):
            self.assertEqual(cache.bounded("inject", directory), ["namespace=" + "d" * 32])
        with patch.object(cache, "worker_command", return_value=[sys.executable, "-c", "raise SystemExit(1)"]), \
                self.assertRaisesRegex(ValueError, "cache worker failed"):
            cache.bounded("extract", directory)

    def test_a_slow_validation_falls_back_cold_inside_the_step_bound(self):
        directory = self.restorable()
        clock = iter([0.0, cache.INJECT_SECONDS - cache.MIN_TRANSFER_SECONDS + 1])
        with patch.object(cache, "validate_bundle"), patch.object(cache.time, "monotonic", side_effect=lambda: next(clock)), \
                patch.object(cache, "transfer") as transfer, self.assertRaisesRegex(ValueError, "budget exhausted"):
            cache.run("inject", directory)
        transfer.assert_not_called()
        self.assertEqual(self.namespaces(), [])
        # The transfer is bounded by what remains of the restore budget.
        directory = self.root / "again"
        directory.mkdir()
        (directory / "state.json").write_text(json.dumps({"prefix": self.prefix}))
        (directory / "data").mkdir()
        clock = iter([0.0, cache.INJECT_SECONDS - 100])
        with patch.object(cache, "validate_bundle"), patch.object(cache.time, "monotonic", side_effect=lambda: next(clock)), \
                patch.object(cache, "transfer") as transfer, contextlib.redirect_stdout(io.StringIO()):
            cache.run("inject", directory)
        self.assertEqual(transfer.call_args.kwargs["timeout"], 100)

    def test_a_cache_miss_publishes_an_unused_namespace(self):
        directory = self.restorable()
        (directory / "data").rmdir()
        with patch.object(cache, "transfer") as transfer, contextlib.redirect_stdout(io.StringIO()):
            cache.run("inject", directory)
        transfer.assert_not_called()
        self.assertEqual(len(self.namespaces()), 1)

    def test_export_reads_the_namespace_the_builds_used(self):
        directory = self.root / "work"
        directory.mkdir()
        (directory / "state.json").write_text(json.dumps({"prefix": self.prefix, "build_namespace": "c" * 32}))
        with patch.object(cache, "transfer") as transfer, patch.object(cache, "seal"):
            cache.run("extract", directory)
        self.assertEqual(transfer.call_args.kwargs["namespace"], "c" * 32)
        # Without a recorded build namespace nothing is exported or reported ready.
        (directory / "state.json").write_text(json.dumps({"prefix": self.prefix}))
        self.output.write_text("")
        with patch.object(cache, "transfer") as transfer, self.assertRaises(KeyError):
            cache.run("extract", directory)
        transfer.assert_not_called()
        self.assertEqual(self.output.read_text(), "")

    def test_cache_outage_cannot_report_a_saved_bundle(self):
        output = self.root / "output"
        output.touch()
        with patch.dict(os.environ, {"GITHUB_OUTPUT": str(output)}), patch.object(cache.sys, "argv", ["cache", "extract"]), \
                patch.object(cache, "context", return_value=self.root), \
                patch.object(cache, "bounded", side_effect=subprocess.TimeoutExpired("docker", 150)), \
                contextlib.redirect_stdout(io.StringIO()):
            cache.main()
        self.assertEqual(output.read_text(), "ready=false\n")

    def test_background_extract_uses_the_build_namespace_without_publisher_credentials(self):
        context = self.root / "context"
        output = self.root / "candidate"
        with patch.dict(os.environ, {"BUILDX_BUILDER": "fixture-builder", "GH_TOKEN": "fixture-token",
                                     "ACTIONS_RUNTIME_TOKEN": "fixture-runtime"}, clear=True), \
                patch.object(cache.subprocess, "Popen") as popen:
            process = cache.start_extract(output, context, namespace="a" * 32, max_content=1024, max_archive=512)
        self.assertIs(process, popen.return_value)
        command = popen.call_args.args[0]
        self.assertIn("BUILDKIT_CACHE_MOUNT_NS=" + "a" * 32, command)
        self.assertIn(f"type=local,dest={output}", command)
        self.assertEqual(command[command.index("--file") + 1], str(context / "Dockerfile"))
        self.assertEqual(command[-1], str(context))
        self.assertIn("max_compressed=512", (context / "Dockerfile").read_text())
        self.assertEqual(popen.call_args.kwargs["env"], {"BUILDX_BUILDER": "fixture-builder"})
        self.assertTrue(popen.call_args.kwargs["start_new_session"])
        self.assertIs(popen.call_args.kwargs["stderr"], subprocess.DEVNULL)


if __name__ == "__main__":
    unittest.main()
