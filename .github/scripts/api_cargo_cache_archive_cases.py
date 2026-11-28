"""Archive and compaction cases for the CI Cargo cache test runner."""
import os
from pathlib import Path
import subprocess
import tarfile
from unittest.mock import patch


class CacheArchiveCases:
    """Reuse the runner's temporary cache and production module."""

    def test_valid_bundle_round_trips(self):
        self.bundle()
        self.assertEqual(self.cache.validate_bundle(self.root, self.prefix)["prefix"], self.prefix)
        manifest = self.root / "manifest.json"
        expected = manifest.read_bytes()
        manifest.unlink()
        with patch.object(self.cache, "digest", wraps=self.cache.digest) as digest:
            self.cache.seal(self.root, self.prefix)
        self.assertEqual([call.args[0].name for call in digest.call_args_list],
                         ["registry.tar.gz", "target.tar.gz"])
        self.assertEqual(manifest.read_bytes(), expected)
        self.cache.validate_bundle(self.root, self.prefix)

    def test_fresh_export_with_invalid_members_does_not_publish_a_manifest(self):
        self.bundle("../outside")
        manifest = self.root / "manifest.json"
        manifest.unlink()
        with self.assertRaisesRegex(ValueError, "archive traversal"):
            self.cache.seal(self.root, self.prefix)
        self.assertFalse(manifest.exists())

    def test_corruption_and_incompatible_bundles_are_rejected(self):
        self.bundle()
        with self.assertRaises(ValueError):
            self.cache.validate_bundle(self.root, "different-prefix-")
        with (self.root / "target.tar.gz").open("ab") as output:
            output.write(b"corruption")
        with self.assertRaisesRegex(ValueError, "checksum"):
            self.cache.validate_bundle(self.root, self.prefix)

    def test_traversal_and_special_files_cannot_reach_cache_mounts(self):
        for member, link in (("../escape", None), ("/absolute", None), ("./x/../../escape", None),
                             ("./link", (tarfile.SYMTYPE, "target")), ("./link", (tarfile.LNKTYPE, "../escape")),
                             ("./device", (tarfile.CHRTYPE, ""))):
            self.bundle(member, link)
            with self.subTest(member=member, link=link), self.assertRaises(ValueError):
                self.cache.validate_bundle(self.root, self.prefix)

    def test_regular_hardlinks_are_allowed(self):
        self.bundle("./release/bin", (tarfile.LNKTYPE, "./release/deps/bin"))
        self.cache.validate_bundle(self.root, self.prefix)

    def test_empty_buildkit_mounts_are_not_saved_as_warm_caches(self):
        self.bundle()
        with tarfile.open(self.root / "target.tar.gz", "w:gz"):
            pass
        self.manifest()
        with self.assertRaisesRegex(ValueError, "empty mount"):
            self.cache.validate_bundle(self.root, self.prefix)

    def test_expansion_archive_and_member_limits_are_enforced(self):
        self.bundle()
        for bound in ("MAX_CONTENT_BYTES", "MAX_ARCHIVE_BYTES", "MAX_MEMBERS"):
            with self.subTest(bound=bound), patch.object(self.cache, bound, 1), self.assertRaises(ValueError):
                self.cache.validate_bundle(self.root, self.prefix)
        (self.root / "oversize").touch()
        with self.assertRaises(ValueError):
            self.cache.seal(self.root, self.prefix)

    def test_caller_bound_replaces_only_the_compressed_archive_limit(self):
        self.bundle()
        size = sum((self.root / f"{name}.tar.gz").stat().st_size for name in self.cache.MOUNTS)
        with patch.object(self.cache, "MAX_ARCHIVE_BYTES", size - 1):
            with self.assertRaisesRegex(ValueError, "compressed cache exceeds"):
                self.cache.validate_bundle(self.root, self.prefix)
            self.cache.validate_bundle(self.root, self.prefix, max_archive=size)
            with self.assertRaisesRegex(ValueError, "compressed cache exceeds"):
                self.cache.seal(self.root, self.prefix)
            self.cache.seal(self.root, self.prefix, max_archive=size)
        for bound in ("MAX_CONTENT_BYTES", "MAX_MEMBERS"):
            with self.subTest(bound=bound), patch.object(self.cache, bound, 1), self.assertRaises(ValueError):
                self.cache.validate_bundle(self.root, self.prefix, max_archive=2 * size)
        # The expanded bound is also the caller's to choose; the default stays 4 GiB.
        with patch.object(self.cache, "MAX_CONTENT_BYTES", 1):
            self.cache.validate_bundle(self.root, self.prefix, max_archive=2 * size, max_content=10 ** 9)
        with self.assertRaisesRegex(ValueError, "expanded cache exceeds"):
            self.cache.validate_bundle(self.root, self.prefix, max_content=1)
        self.assertIn(f"-gt {6 * 1024 ** 2} ]", self.cache.dockerfile("extract"))
        self.assertIn(f"-gt {3 * 1024 ** 2} ]", self.cache.dockerfile("extract", 3 * 1024 ** 3))
        with patch.object(self.cache.subprocess, "run") as run:
            self.cache.transfer("inject", self.root)
            self.assertEqual(run.call_args.kwargs["timeout"], 150)
            self.cache.transfer("extract", self.root, timeout=360, max_content=6 * 1024 ** 3)
            self.assertEqual(run.call_args.kwargs["timeout"], 360)
            self.assertIn(f"-gt {6 * 1024 ** 2} ]", run.call_args.kwargs["input"])

    def test_am_compaction_keeps_the_cache_and_target_bounds(self):
        bound = 6 * 1024 ** 3
        ordinary = self.cache.dockerfile("extract", bound)
        compact = self.cache.dockerfile("extract", bound, compact_registry=True)
        self.assertNotIn("--exclude=./src", ordinary)
        self.assertIn("--exclude=./src", compact)
        self.assertIn(f"max_expanded={bound}", compact)
        self.assertIn(f"max_compressed={self.cache.MAX_ARCHIVE_BYTES}", compact)
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
        script = self.cache.export_script(100000, 40000, 200)
        subprocess.run(["bash", "-c", script, "export", str(registry), str(target), str(output)],
                       env={**os.environ, "COPYFILE_DISABLE": "1"}, check=True)
        self.assertEqual((output / "compacted").read_text().strip(), "2")
        (output / "compacted").unlink()
        self.cache.seal(output, self.prefix, family="example", max_archive=40000, max_content=100000)
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
                script = self.cache.export_script(1000000, max_compressed, max_members)
                subprocess.run(["bash", "-c", script, "export", str(registry), str(target), str(output)],
                       env={**os.environ, "COPYFILE_DISABLE": "1"}, check=True)
                self.assertEqual((output / "compacted").read_text().strip(), "1")
                (output / "compacted").unlink()
                with patch.object(self.cache, "MAX_MEMBERS", max_members):
                    self.cache.seal(output, self.prefix, family="example", max_archive=max_compressed, max_content=1000000)
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
        script = self.cache.export_script(400000, 140000, 500)
        subprocess.run(["bash", "-c", script, "export", str(registry), str(target), str(output)], env=env, check=True)
        self.assertLessEqual(len(calls.read_text().splitlines()), 5)
        self.assertTrue((output / "compacted").exists())
        (output / "compacted").unlink()
        self.cache.seal(output, self.prefix, family="example", max_archive=140000, max_content=400000)

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
        script = self.cache.export_script(1000000, 32000, 100)
        subprocess.run(["bash", "-c", script, "export", str(registry), str(target), str(output)],
                       env={**os.environ, "COPYFILE_DISABLE": "1"}, check=True)
        self.assertEqual((output / "compacted").read_text().strip(), "2")
        (output / "compacted").unlink()
        self.cache.seal(output, self.prefix, family="example", max_archive=32000, max_content=1000000)
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
        script = self.cache.export_script(40000, 20000, 100)
        subprocess.run(["bash", "-c", script, "export", str(registry), str(target), str(output)],
                       env={**os.environ, "COPYFILE_DISABLE": "1"}, check=True)
        self.assertEqual(sorted(path.name for path in output.iterdir()), ["empty"])
        with self.assertRaisesRegex(ValueError, "empty mount"):
            self.cache.seal(output, self.prefix)

    def test_archive_symlink_and_extra_files_are_rejected(self):
        self.bundle()
        (self.root / "unexpected").touch()
        with self.assertRaises(ValueError):
            self.cache.validate_bundle(self.root, self.prefix)
        (self.root / "unexpected").unlink()
        target = self.root / "target.tar.gz"
        target.unlink()
        target.symlink_to("registry.tar.gz")
        with self.assertRaises(ValueError):
            self.cache.validate_bundle(self.root, self.prefix)
