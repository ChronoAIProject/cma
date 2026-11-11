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
        for name in ("infra/api/Dockerfile", "api/Cargo.lock", "api/Cargo.toml", "api/rust-toolchain.toml",
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

    def test_release_push_prepares_and_injects_without_cache_write_authority(self):
        self.inputs()
        event_path, output = self.root / "event.json", self.root / "output"
        event_path.write_text(json.dumps({"repository": {"full_name": cache.REPOSITORY}}))
        env = dict(self.env, GITHUB_EVENT_NAME="push", GITHUB_REF="refs/heads/tl/release/train-w2",
                   GITHUB_EVENT_PATH=str(event_path), RUNNER_TEMP=str(self.root), GITHUB_OUTPUT=str(output))
        with patch.dict(os.environ, env, clear=True), patch.object(cache, "ROOT", self.root), \
                patch.object(cache, "transfer") as transfer:
            with patch.object(cache.sys, "argv", ["cache", "prepare"]):
                cache.main()
            self.assertIn("key=cma-api-cargo-", output.read_text())
            bundle = self.root / "cma-api-cargo-cache/data"
            original_root = self.root
            self.root = bundle
            self.bundle()
            self.root = original_root
            state = self.root / "cma-api-cargo-cache/state.json"
            state.write_text(json.dumps({"prefix": self.prefix}))
            with patch.object(cache.sys, "argv", ["cache", "inject"]), contextlib.redirect_stdout(io.StringIO()):
                cache.main()
            transfer.assert_called_once_with("inject", bundle.resolve())
            transfer.reset_mock()
            before = output.read_text()
            with patch.object(cache.sys, "argv", ["cache", "extract"]), \
                    self.assertRaisesRegex(ValueError, "only develop push"):
                cache.main()
            transfer.assert_not_called()
            self.assertEqual(output.read_text(), before)

    def test_cache_write_authority_remains_develop_only(self):
        develop = dict(self.env, GITHUB_EVENT_NAME="push", GITHUB_REF="refs/heads/develop")
        cache.authority(develop, self.event, "extract")
        for ref in ("refs/heads/tl/release/train", "refs/heads/tl/release/nested/train"):
            release = dict(develop, GITHUB_REF=ref)
            for operation in ("prepare", "inject"):
                cache.authority(release, self.event, operation)
            with self.subTest(ref=ref), self.assertRaisesRegex(ValueError, "only develop push"):
                cache.authority(release, self.event, "extract")
        with self.assertRaisesRegex(ValueError, "only develop push"):
            cache.authority(self.env, self.event, "extract")
        for ref in ("refs/heads/main", "refs/heads/tl/release", "refs/heads/tl/release/",
                    "refs/heads/tl/release-other/train", "refs/tags/tl/release/train"):
            with self.subTest(ref=ref), self.assertRaises(ValueError):
                cache.authority(dict(develop, GITHUB_REF=ref), self.event)
        release = dict(develop, GITHUB_REF="refs/heads/tl/release/train")
        foreign = {"repository": {"full_name": "other/project"}}
        with self.assertRaises(ValueError):
            cache.authority(release, foreign)
        with self.assertRaises(ValueError):
            cache.authority(dict(release, GITHUB_REPOSITORY="other/project"), self.event)
        with self.assertRaises(ValueError):
            cache.authority(release, self.event, "unknown")

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
                     "infra/api/Dockerfile", ".github/scripts/api-cargo-cache.py"):
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

    def test_bridge_mount_ids_match_the_real_api_dockerfile(self):
        definition = (cache.ROOT / "infra/api/Dockerfile").read_text()
        self.assertIn(f'id={cache.MOUNTS["registry"]},target=/usr/local/cargo/registry,sharing=locked', definition)
        self.assertIn('id=cma-api-target-${TARGETARCH},target=/build/target,sharing=locked', definition)
        self.assertEqual(cache.MOUNTS["target"], "cma-api-target-amd64")

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


if __name__ == "__main__":
    unittest.main()
