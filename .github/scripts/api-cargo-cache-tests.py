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
import tarfile
import tempfile
import unittest
from unittest.mock import patch


SPEC = importlib.util.spec_from_file_location("api_cargo_cache", Path(__file__).with_name("api-cargo-cache.py"))
cache = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(cache)


class CacheTests(unittest.TestCase):
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

    def test_valid_bundle_round_trips(self):
        self.bundle()
        self.assertEqual(cache.validate_bundle(self.root, self.prefix)["prefix"], self.prefix)
        cache.seal(self.root, self.prefix)
        cache.validate_bundle(self.root, self.prefix)

    def test_corruption_and_incompatible_bundles_are_rejected(self):
        self.bundle()
        with self.assertRaises(ValueError):
            cache.validate_bundle(self.root, "different-prefix-")
        with (self.root / "target.tar.gz").open("ab") as output:
            output.write(b"corruption")
        with self.assertRaisesRegex(ValueError, "checksum"):
            cache.validate_bundle(self.root, self.prefix)

    def test_traversal_and_special_files_cannot_reach_cache_mounts(self):
        for member, link in (("../escape", None), ("/absolute", None), ("./x/../../escape", None),
                             ("./link", (tarfile.SYMTYPE, "target")), ("./link", (tarfile.LNKTYPE, "../escape")),
                             ("./device", (tarfile.CHRTYPE, ""))):
            self.bundle(member, link)
            with self.subTest(member=member, link=link), self.assertRaises(ValueError):
                cache.validate_bundle(self.root, self.prefix)

    def test_regular_hardlinks_are_allowed(self):
        self.bundle("./release/bin", (tarfile.LNKTYPE, "./release/deps/bin"))
        cache.validate_bundle(self.root, self.prefix)

    def test_empty_buildkit_mounts_are_not_saved_as_warm_caches(self):
        self.bundle()
        with tarfile.open(self.root / "target.tar.gz", "w:gz"):
            pass
        self.manifest()
        with self.assertRaisesRegex(ValueError, "empty mount"):
            cache.validate_bundle(self.root, self.prefix)

    def test_expansion_archive_and_member_limits_are_enforced(self):
        self.bundle()
        for bound in ("MAX_CONTENT_BYTES", "MAX_ARCHIVE_BYTES", "MAX_MEMBERS"):
            with self.subTest(bound=bound), patch.object(cache, bound, 1), self.assertRaises(ValueError):
                cache.validate_bundle(self.root, self.prefix)
        (self.root / "oversize").touch()
        with self.assertRaises(ValueError):
            cache.seal(self.root, self.prefix)

    def test_caller_bound_replaces_only_the_compressed_archive_limit(self):
        self.bundle()
        size = sum((self.root / f"{name}.tar.gz").stat().st_size for name in cache.MOUNTS)
        with patch.object(cache, "MAX_ARCHIVE_BYTES", size - 1):
            with self.assertRaisesRegex(ValueError, "compressed cache exceeds"):
                cache.validate_bundle(self.root, self.prefix)
            cache.validate_bundle(self.root, self.prefix, max_archive=size)
            with self.assertRaisesRegex(ValueError, "compressed cache exceeds"):
                cache.seal(self.root, self.prefix)
            cache.seal(self.root, self.prefix, max_archive=size)
        for bound in ("MAX_CONTENT_BYTES", "MAX_MEMBERS"):
            with self.subTest(bound=bound), patch.object(cache, bound, 1), self.assertRaises(ValueError):
                cache.validate_bundle(self.root, self.prefix, max_archive=2 * size)
        # The expanded bound is also the caller's to choose; the default stays 4 GiB.
        with patch.object(cache, "MAX_CONTENT_BYTES", 1):
            cache.validate_bundle(self.root, self.prefix, max_archive=2 * size, max_content=10 ** 9)
        with self.assertRaisesRegex(ValueError, "expanded cache exceeds"):
            cache.validate_bundle(self.root, self.prefix, max_content=1)
        self.assertIn(f"-gt {4 * 1024 ** 2} ]", cache.dockerfile("extract"))
        self.assertIn(f"-gt {6 * 1024 ** 2} ]", cache.dockerfile("extract", 6 * 1024 ** 3))
        with patch.object(cache.subprocess, "run") as run:
            cache.transfer("inject", self.root)
            self.assertEqual(run.call_args.kwargs["timeout"], 150)
            cache.transfer("extract", self.root, timeout=360, max_content=6 * 1024 ** 3)
            self.assertEqual(run.call_args.kwargs["timeout"], 360)
            self.assertIn(f"-gt {6 * 1024 ** 2} ]", run.call_args.kwargs["input"])

    def test_am_compaction_keeps_the_cache_and_target_bounds(self):
        bound = 6 * 1024 ** 3
        ordinary = cache.dockerfile("extract", bound)
        compact = cache.dockerfile("extract", bound, compact_registry=True)
        self.assertNotIn("--exclude=./src", ordinary)
        self.assertIn("--exclude=./src", compact)
        self.assertIn(f"max_expanded={bound}", compact)
        self.assertIn(f"max_compressed={cache.MAX_ARCHIVE_BYTES}", compact)
        self.assertIn('archive_stream "$registry" "$registry_archive" "$max_expanded" "$max_compressed"', compact)
        self.assertIn('archive_stream "$target" "$target_archive" "$remaining_expanded" "$remaining_compressed"', compact)
        self.assertNotIn("rm -rf /cache", compact)

    def test_am_compaction_restores_complete_groups_and_cross_group_links(self):
        import os
        import random

        registry = self.root / "registry"
        target = self.root / "target"
        output = self.root / "export"
        registry.mkdir()
        (registry / "package.crate").write_bytes(b"package archive")
        random_source = random.Random(42)

        def group(name, size):
            directory = target / "release" / "incremental" / name / "s-complete"
            directory.mkdir(parents=True)
            (directory / "work-products.bin").write_bytes(random_source.randbytes(size))
            return directory

        am = group("cma-111", 16000)
        core = group("layer2_core-222", 4000)
        large = group("large-333", 200000)
        unfinished = group("unfinished-444", 100)
        (unfinished.parent / "s-aborted-working").mkdir()
        os.link(large / "work-products.bin", large / "another-name")
        # Link to a small regular file whose other path is in the omitted group.
        (large / "shared").write_bytes(b"shared object")
        os.link(large / "shared", am / "linked-object")
        os.link(am / "work-products.bin", core / "linked-retained")
        before = {path.relative_to(target): path.read_bytes() for path in target.rglob("*") if path.is_file()}
        script = cache.export_script(100000, 40000, 200)
        subprocess.run(["bash", "-c", script, "export", str(registry), str(target), str(output)],
                       env={**os.environ, "COPYFILE_DISABLE": "1"}, check=True)
        self.assertEqual((output / "compacted").read_text().strip(), "2")
        (output / "compacted").unlink()
        cache.seal(output, self.prefix, family="example", max_archive=40000, max_content=100000)
        restored = self.root / "restored"
        restored.mkdir()
        restored_registry = self.root / "restored-registry"
        restored_registry.mkdir()
        subprocess.run(["tar", "-xzpf", str(output / "registry.tar.gz"), "-C", str(restored_registry)], check=True)
        subprocess.run(["tar", "-xzpf", str(output / "target.tar.gz"), "-C", str(restored)], check=True)
        self.assertEqual((restored_registry / "package.crate").read_bytes(), b"package archive")
        for selected in (am, core):
            for path in selected.iterdir():
                self.assertEqual((restored / path.relative_to(target)).read_bytes(), path.read_bytes())
        self.assertFalse((restored / large.parent.relative_to(target)).exists())
        self.assertFalse((restored / unfinished.parent.relative_to(target)).exists())
        self.assertEqual(before, {path.relative_to(target): path.read_bytes() for path in target.rglob("*") if path.is_file()})

    def test_am_compaction_obeys_compressed_and_member_limits_independently(self):
        import random

        registry = self.root / "registry"
        target = self.root / "target"
        registry.mkdir()
        (registry / "package.crate").write_bytes(b"package archive")
        random_source = random.Random(815)
        for name in ("cma-111", "layer2_core-222", "other-333"):
            directory = target / "release/incremental" / name / "s-complete"
            directory.mkdir(parents=True)
            (directory / "work-products.bin").write_bytes(random_source.randbytes(12000))
        for tag, max_compressed, max_members in (("compressed", 20000, 100), ("members", 1000000, 6)):
            with self.subTest(tag=tag):
                output = self.root / tag
                script = cache.export_script(1000000, max_compressed, max_members)
                subprocess.run(["bash", "-c", script, "export", str(registry), str(target), str(output)],
                       env={**os.environ, "COPYFILE_DISABLE": "1"}, check=True)
                self.assertEqual((output / "compacted").read_text().strip(), "1")
                (output / "compacted").unlink()
                with patch.object(cache, "MAX_MEMBERS", max_members):
                    cache.seal(output, self.prefix, family="example", max_archive=max_compressed, max_content=1000000)
                with tarfile.open(output / "target.tar.gz") as archive:
                    self.assertTrue(all("cma-111" in member.name for member in archive))

    def test_many_candidates_bound_the_number_of_archive_compressions(self):
        import random
        import shutil

        registry, target, output = (self.root / name for name in ("registry", "target", "export"))
        registry.mkdir()
        (registry / "package.crate").write_bytes(b"package")
        source = random.Random(3358)
        for number in range(30):
            group = target / "release/incremental" / f"crate-{number:02d}" / "s-final"
            group.mkdir(parents=True)
            (group / "work-products.bin").write_bytes(source.randbytes(32768))
        bin_dir = self.root / "bin"
        bin_dir.mkdir()
        calls = self.root / "gzip-calls"
        gzip = shutil.which("gzip")
        self.assertIsNotNone(gzip)
        wrapper = bin_dir / "gzip"
        wrapper.write_text('#!/bin/sh\n[ "$1" != "-1" ] || echo compress >> "$COUNT_FILE"\nexec "$REAL_GZIP" "$@"\n')
        wrapper.chmod(0o755)
        env = {**os.environ, "COPYFILE_DISABLE": "1", "PATH": f"{bin_dir}:{os.environ['PATH']}",
               "COUNT_FILE": str(calls), "REAL_GZIP": gzip}
        script = cache.export_script(400000, 140000, 500)
        subprocess.run(["bash", "-c", script, "export", str(registry), str(target), str(output)], env=env, check=True)
        self.assertLessEqual(len(calls.read_text().splitlines()), 5)
        self.assertTrue((output / "compacted").exists())
        (output / "compacted").unlink()
        cache.seal(output, self.prefix, family="example", max_archive=140000, max_content=400000)

    def test_gateway_compaction_keeps_the_executable_and_gateway_crates_first(self):
        import random

        registry, target, output = (self.root / name for name in ("registry", "target", "export"))
        registry.mkdir()
        (registry / "package.crate").write_bytes(b"package")
        source = random.Random(2098)
        groups = ("aaa-other-111", "cmaeg-222", "cmaeg_core-333")
        for name in groups:
            group = target / "release/incremental" / name / "s-final"
            group.mkdir(parents=True)
            (group / "work-products.bin").write_bytes(source.randbytes(12000))
        script = cache.export_script(1000000, 32000, 100)
        subprocess.run(["bash", "-c", script, "export", str(registry), str(target), str(output)],
                       env={**os.environ, "COPYFILE_DISABLE": "1"}, check=True)
        self.assertEqual((output / "compacted").read_text().strip(), "2")
        (output / "compacted").unlink()
        cache.seal(output, self.prefix, family="example", max_archive=32000, max_content=1000000)
        with tarfile.open(output / "target.tar.gz") as archive:
            self.assertEqual({Path(member.name).parts[2] for member in archive}, set(groups[1:]))

    def test_am_compaction_refuses_when_no_complete_group_fits(self):
        registry = self.root / "registry"
        target = self.root / "target"
        output = self.root / "export"
        registry.mkdir()
        (registry / "package.crate").write_bytes(b"package archive")
        group = target / "release/incremental/cma-111/s-complete"
        group.mkdir(parents=True)
        (group / "work-products.bin").write_bytes(b"x" * 100000)
        script = cache.export_script(40000, 20000, 100)
        subprocess.run(["bash", "-c", script, "export", str(registry), str(target), str(output)],
                       env={**os.environ, "COPYFILE_DISABLE": "1"}, check=True)
        self.assertEqual(sorted(path.name for path in output.iterdir()), ["empty"])
        with self.assertRaisesRegex(ValueError, "empty mount"):
            cache.seal(output, self.prefix)

    def test_archive_symlink_and_extra_files_are_rejected(self):
        self.bundle()
        (self.root / "unexpected").touch()
        with self.assertRaises(ValueError):
            cache.validate_bundle(self.root, self.prefix)
        (self.root / "unexpected").unlink()
        target = self.root / "target.tar.gz"
        target.unlink()
        target.symlink_to("registry.tar.gz")
        with self.assertRaises(ValueError):
            cache.validate_bundle(self.root, self.prefix)

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
            with self.subTest(archives=archives), \
                    patch.object(cache.sys, "argv", ["cache", "inject"]), \
                    patch.object(cache, "context", return_value=directory), \
                    patch.object(cache, "transfer") as transfer, \
                    contextlib.redirect_stdout(io.StringIO()) as output:
                cache.main()
            transfer.assert_not_called()
            self.assertIn("::warning::API Cargo cache inject unavailable (ValueError)", output.getvalue())

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

    def test_cache_outage_cannot_report_a_saved_bundle(self):
        output = self.root / "output"
        output.touch()
        with patch.dict(os.environ, {"GITHUB_OUTPUT": str(output)}), patch.object(cache.sys, "argv", ["cache", "extract"]), \
                patch.object(cache, "context", return_value=self.root), \
                patch.object(cache, "run", side_effect=subprocess.TimeoutExpired("docker", 150)), \
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
