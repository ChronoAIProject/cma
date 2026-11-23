import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import patch

from staging_cd_build_cache import bridge

SCRIPT = Path(__file__).with_name("staging_cd_cargo_select.sh")
PREFIX = "cma-cd-cargo-v4-am-" + "a" * 64 + "-" + "b" * 64 + "-"
FAMILY = "cma-cd-cargo-v4-am-" + "a" * 64 + "-"


class SelectionTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="cargo selection ")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.registry, self.target = self.root / "registry", self.root / "target"
        self.registry.mkdir()
        (self.registry / "index").write_bytes(b"registry")
        (self.target / "release/incremental").mkdir(parents=True)
        (self.target / ".rustc_info.json").write_text("{}")
        self.tool = bridge()
        self.serial = 0

    def unit(self, name, size, *, state="s-complete"):
        directory = self.target / "release/incremental" / name / state
        directory.mkdir(parents=True)
        (directory / "work-products.bin").write_bytes(b"receipt")
        (directory / "products.o").write_bytes(b"x" * size)
        return directory

    def inventory(self):
        return {str(p.relative_to(self.root)): (p.stat().st_size, hashlib.sha256(p.read_bytes()).hexdigest())
                for root in (self.registry, self.target) for p in root.rglob("*") if p.is_file()}

    def select(self, limit=1000, members=200_000, *, succeeds=True):
        self.serial += 1
        output = self.root / f"output-{self.serial}"
        result = subprocess.run(["bash", str(SCRIPT), str(self.registry), str(self.target),
                                 str(output), str(limit), str(members)],
                                capture_output=True, text=True, timeout=10,
                                env=dict(os.environ, COPYFILE_DISABLE="1"))
        if succeeds:
            self.assertEqual(result.returncode, 0, result.stderr)
        else:
            self.assertNotEqual(result.returncode, 0)
        return output

    def validate(self, output, limit, members=200_000):
        with patch.object(self.tool, "MAX_MEMBERS", members):
            self.tool.seal(output, PREFIX, family=FAMILY, max_content=limit)
        with tarfile.open(output / "target.tar.gz") as archive:
            return {item.name: archive.extractfile(item).read() for item in archive if item.isfile()}

    def test_under_bound_keeps_all_bytes_and_source_mounts(self):
        self.unit("first-1", 40)
        self.unit("second-2", 30)
        before = self.inventory()
        output = self.select()
        contents = self.validate(output, 1000)
        self.assertEqual(len(contents), 5)
        self.assertEqual(contents["./release/incremental/first-1/s-complete/products.o"], b"x" * 40)
        self.assertEqual(self.inventory(), before)
        with tarfile.open(output / "registry.tar.gz") as archive:
            self.assertEqual(archive.extractfile("./index").read(), b"registry")

    def test_oversize_retains_whole_units_and_skips_one_that_cannot_fit(self):
        self.unit("too_big-1", 1000)
        self.unit("useful-2", 70)
        self.unit("smaller-3", 60)
        before = self.inventory()
        output = self.select(100)
        contents = self.validate(output, 100)
        self.assertEqual(set(contents), {"./.rustc_info.json",
                                        "./release/incremental/useful-2/s-complete/work-products.bin",
                                        "./release/incremental/useful-2/s-complete/products.o"})
        self.assertEqual(self.inventory(), before)

    def test_equal_sizes_have_stable_path_order_across_creation_order(self):
        self.unit("zeta-1", 50)
        self.unit("alpha-2", 50)
        first = self.validate(self.select(70), 70)
        for name in ("zeta-1", "alpha-2"):
            shutil.rmtree(self.target / "release/incremental" / name)
        self.unit("alpha-2", 50)
        self.unit("zeta-1", 50)
        second = self.validate(self.select(70), 70)
        self.assertEqual(first, second)
        self.assertTrue(any("alpha-2" in name for name in first))
        self.assertFalse(any("zeta-1" in name for name in first))

    def test_registry_or_all_units_over_budget_still_withhold(self):
        self.unit("large-1", 1000)
        for limit in (5, 100):
            with self.subTest(limit=limit):
                output = self.select(limit)
                self.assertEqual({p.name for p in output.iterdir()}, {"oversize"})
                with self.assertRaisesRegex(ValueError, "expanded cache exceeds"):
                    self.tool.seal(output, PREFIX, family=FAMILY, max_content=limit)

    def test_incomplete_units_are_never_used_to_rescue_oversize_state(self):
        self.unit("working-1", 50, state="s-current-working")
        incomplete = self.unit("incomplete-2", 50)
        (incomplete / "work-products.bin").unlink()
        self.unit("useful-3", 30)
        contents = self.validate(self.select(60), 60)
        self.assertTrue(any("useful-3" in name for name in contents))
        self.assertFalse(any("working-1" in name or "incomplete-2" in name for name in contents))

    def test_symlink_and_special_files_refuse_even_inside_an_omitted_unit(self):
        directory = self.unit("large-1", 1000)
        self.unit("small-2", 10)
        path = directory / "bad"
        path.symlink_to(self.registry / "index")
        self.select(60, succeeds=False)
        path.unlink()
        os.mkfifo(path)
        self.select(60, succeeds=False)

    def test_unknown_target_layout_is_not_silently_filtered(self):
        self.unit("large-1", 1000)
        self.unit("small-2", 10)
        (self.target / "release/compiled-binary").write_bytes(b"workspace output")
        self.select(60, succeeds=False)

    def test_sparse_logical_bytes_and_hardlinks_cannot_exceed_the_bound(self):
        huge = self.unit("sparse-1", 0)
        with (huge / "products.o").open("wb") as stream:
            stream.truncate(32 * 1024 * 1024)
        unit = self.unit("linked-2", 35)
        os.link(unit / "products.o", unit / "second.o")
        self.unit("small-3", 10)
        contents = self.validate(self.select(60), 60)
        self.assertTrue(any("small-3" in name for name in contents))
        self.assertFalse(any("sparse-1" in name or "linked-2" in name for name in contents))

    def test_member_and_compressed_limits_remain_independent(self):
        self.unit("first-1", 10)
        self.unit("second-2", 10)
        contents = self.validate(self.select(1000, 7), 1000, 7)
        self.assertEqual(len(contents), 3)
        output = self.select()
        with self.assertRaisesRegex(ValueError, "compressed cache exceeds"):
            self.tool.seal(output, PREFIX, family=FAMILY, max_archive=1)

    def test_only_explicit_extraction_can_select_incremental_units(self):
        default = self.tool.dockerfile("extract", 1000)
        self.assertNotIn("CARGO_SUBSET", default)
        selected = self.tool.dockerfile("extract", 1000, incremental_subset=True)
        self.assertIn(SCRIPT.read_text(), selected)
        for operation in ("inject", "inject-target"):
            with self.subTest(operation=operation), self.assertRaises(ValueError):
                self.tool.dockerfile(operation, incremental_subset=True)


if __name__ == "__main__":
    unittest.main()
