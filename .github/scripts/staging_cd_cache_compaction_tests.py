"""Bounded Cargo exports prioritize distinct workspace crates over old generations."""
import os
from pathlib import Path
import random
import subprocess
import tarfile
import tempfile
import unittest

from staging_cd_cache_compaction import export_script


class CargoCompactionTests(unittest.TestCase):
    def test_newest_executable_and_distinct_dependencies_win_under_pressure(self):
        with tempfile.TemporaryDirectory(prefix="cma-cargo-priority-") as directory:
            root = Path(directory)
            registry, target, output = (root / name for name in ("registry", "target", "export"))
            registry.mkdir()
            (registry / "package.crate").write_bytes(b"package")
            random_source = random.Random(42)
            groups = {"cma-111": (24000, 1), "cma-222": (12000, 2),
                      "layer2_core-111": (12000, 2), "layer1_public-111": (9000, 2)}
            for name, (size, modified) in groups.items():
                work_products = target / "release" / "incremental" / name / "s-final" / "work-products.bin"
                work_products.parent.mkdir(parents=True)
                work_products.write_bytes(random_source.randbytes(size))
                os.utime(work_products, (modified, modified))

            script = export_script(1024 * 1024, 40500, 100)
            subprocess.run(["bash", "-c", script, "export", str(registry), str(target), str(output)],
                           env={**os.environ, "COPYFILE_DISABLE": "1"}, check=True)
            self.assertEqual((output / "compacted").read_text().strip(), "3")
            with tarfile.open(output / "target.tar.gz") as archive:
                selected = {Path(member.name).parts[2] for member in archive}
            self.assertEqual(selected, set(groups) - {"cma-111"})


if __name__ == "__main__":
    unittest.main()
