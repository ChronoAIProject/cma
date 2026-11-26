"""Bounded Cargo exports prioritize distinct workspace crates over old generations."""
import os
from pathlib import Path
import random
import shutil
import subprocess
import tarfile
import tempfile
import unittest

from staging_cd_cache_compaction import export_script


class CargoCompactionTests(unittest.TestCase):
    def test_counts_members_without_reopening_the_archive(self):
        with tempfile.TemporaryDirectory(prefix="cma-cargo-member-count-") as directory:
            root = Path(directory)
            registry, target, output, refused = (root / name for name in ("registry", "target", "export", "refused"))
            registry.mkdir()
            target.mkdir()
            (registry / "package.crate").write_bytes(b"package")
            os.link(registry / "package.crate", registry / "package-copy.crate")
            (target / ".rustc_info.json").write_text("{}")

            tar = root / "tar"
            tar.write_text('''#!/bin/bash
if [ "$3" != "-cvf" ]; then exit 9; fi
exec "$REAL_TAR" "$@"
''')
            tar.chmod(0o755)
            environment = {**os.environ, "COPYFILE_DISABLE": "1", "PATH": f"{root}:{os.environ['PATH']}",
                           "REAL_TAR": shutil.which("tar")}

            def run_export(destination, members):
                script = export_script(1024 * 1024, 1024 * 1024, members)
                subprocess.run(["bash", "-c", script, "export", str(registry), str(target), str(destination)],
                               env=environment, check=True, timeout=10)

            run_export(output, 5)
            counts = []
            for archive_name in ("registry.tar.gz", "target.tar.gz"):
                with tarfile.open(output / archive_name) as archive:
                    counts.append(len(archive.getmembers()))
            self.assertEqual(counts, [3, 2])

            run_export(refused, 2)
            self.assertEqual({path.name for path in refused.iterdir()}, {"oversize"})

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

            # A filesystem can charge separately allocated directory blocks
            # to each group. Those blocks are not payload in a tar stream.
            du = root / "du"
            du.write_text('''#!/bin/bash
case "$2" in
    */release/incremental/*) "$REAL_DU" "$@" | awk '{print ($1 + 12) "\\t" $2}' ;;
    *) exec "$REAL_DU" "$@" ;;
esac
''')
            du.chmod(0o755)
            environment = {**os.environ, "COPYFILE_DISABLE": "1", "PATH": f"{root}:{os.environ['PATH']}",
                           "REAL_DU": shutil.which("du")}
            script = export_script(1024 * 1024, 40500, 100)
            subprocess.run(["bash", "-c", script, "export", str(registry), str(target), str(output)],
                           env=environment, check=True)
            self.assertEqual((output / "compacted").read_text().strip(), "3")
            with tarfile.open(output / "target.tar.gz") as archive:
                selected = {Path(member.name).parts[2] for member in archive}
            self.assertEqual(selected, set(groups) - {"cma-111"})


    def test_oversized_attempts_stop_streaming_before_compressing_every_group(self):
        with tempfile.TemporaryDirectory(prefix="cma-cargo-budget-") as directory:
            root = Path(directory)
            registry, target, output = (root / name for name in ("registry", "target", "export"))
            registry.mkdir()
            (registry / "package.crate").write_bytes(b"package")
            random_source = random.Random(3873)
            mib = 1024 * 1024
            group_count = 6
            for group in range(group_count):
                state = target / "release" / "incremental" / f"crate{group}-seed" / "s-final"
                state.mkdir(parents=True)
                (state / "work-products.bin").write_bytes(random_source.randbytes(mib))

            gzip = root / "gzip"
            gzip.write_text('''#!/bin/bash
set -eu -o pipefail
if [ "$1" != "-1" ]; then exec "$REAL_GZIP" "$@"; fi
work=$(mktemp -d "$PROBE_TMP/gzip.XXXXXX")
trap 'rm -f "$work/input" "$work/count"; rmdir "$work"' EXIT
mkfifo "$work/input"
wc -c < "$work/input" > "$work/count" &
counter=$!
status=0
tee "$work/input" | "$REAL_GZIP" "$@" || status=$?
wait "$counter"
cat "$work/count" >> "$PROBE_TMP/bytes"
exit "$status"
''')
            gzip.chmod(0o755)
            real_gzip = shutil.which("gzip")
            self.assertIsNotNone(real_gzip)
            environment = {**os.environ, "COPYFILE_DISABLE": "1", "PATH": f"{root}:{os.environ['PATH']}",
                           "REAL_GZIP": real_gzip, "PROBE_TMP": directory}
            script = export_script(12 * mib, 5 * mib // 2, 100)
            process = subprocess.run(["bash", "-c", script, "export", str(registry), str(target), str(output)],
                                     env=environment, capture_output=True, text=True, timeout=30)
            self.assertEqual(process.returncode, 0, process.stderr)
            input_sizes = [int(size) for size in (root / "bytes").read_text().splitlines()]
            # Pipes can queue raw bytes after gzip reaches the output limit.
            # No attempt should process the complete oversized workspace.
            self.assertLess(max(input_sizes), group_count * mib)
            self.assertLess(sum(input_sizes), 8 * mib)
            self.assertLessEqual(sum(path.stat().st_size for path in output.glob("*.tar.gz")), 5 * mib // 2)
            self.assertEqual((output / "compacted").read_text().strip(), "2")
            with tarfile.open(output / "target.tar.gz") as archive:
                selected = {Path(member.name).parts[2] for member in archive}
            self.assertEqual(selected, {"crate0-seed", "crate1-seed"})


if __name__ == "__main__":
    unittest.main()
