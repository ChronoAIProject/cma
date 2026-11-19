#!/usr/bin/env python3
"""Build a real workspace to expose stale artifacts and unnecessary recompiles."""

from contextlib import redirect_stdout
import importlib.util
from io import StringIO
import os
from pathlib import Path
import subprocess
import tempfile
import time
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("workspace_cache", ROOT / "api/build_support/workspace_cache.py")
cache = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(cache)
OLD = (100, 100)


class CargoReuseTests(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name).resolve()
        self.target = self.root / "target"
        self.plan = self.root.parent / (self.root.name + "-plan.json")
        self.addCleanup(self.plan.unlink, missing_ok=True)
        self.enterContext(patch.dict(os.environ, {"CARGO_INCREMENTAL": "1", "CHRONO_BUILD_SHA": "first"}))
        self.write("Cargo.toml", '[workspace]\nresolver = "2"\nmembers = ["cma", "middle", "leaf", "untouched"]\n')
        for name, deps in {
            "cma": ("middle", "untouched"), "middle": ("leaf",), "leaf": (), "untouched": (),
        }.items():
            manifest = f'[package]\nname = "{name}"\nversion = "0.1.0"\nedition = "2021"\n[dependencies]\n'
            self.write(f"{name}/Cargo.toml", manifest + "".join(f'{dep} = {{path = "../{dep}"}}\n' for dep in deps))
        self.write("cma/src/main.rs", 'fn main() { println!("{}:{}:{}", env!("CHRONO_BUILD_SHA"), middle::value(), untouched::value()); }\n')
        self.write("middle/src/lib.rs", "pub fn value() -> &'static str { leaf::value() }\n")
        self.write("leaf/src/lib.rs", 'pub fn value() -> &\'static str { include_str!("value.txt") }\n')
        self.write("leaf/src/value.txt", "old")
        self.write("untouched/src/lib.rs", "pub fn value() -> u8 { 7 }\n")
        self.command("cargo", "generate-lockfile", "--offline")

    def write(self, path, content):
        file = self.root / path
        file.parent.mkdir(parents=True, exist_ok=True)
        file.write_text(content)
        os.utime(file, OLD)
        return file

    def command(self, *args):
        return subprocess.run(args, cwd=self.root, capture_output=True, text=True, check=True, timeout=60)

    def build(self, stamp):
        os.environ["CHRONO_BUILD_SHA"] = stamp
        with redirect_stdout(StringIO()) as notes:
            cache.prepare(self.root, self.target, "cma", self.plan)
        invocation = self.command("cargo", "build", "--release", "--locked", "--offline", "--verbose", "--bin", "cma")
        output = self.command(str(self.target / "release" / "cma")).stdout.strip()
        cache.finish(self.target, self.plan, "cma")
        return notes.getvalue(), invocation.stderr, output

    def test_archive_timestamp_does_not_hide_transitive_input_change(self):
        _, _, seeded = self.build("first")
        self.assertEqual(seeded, "first:old:7")
        self.write("leaf/src/value.txt", "new")
        notes, rust, output = self.build("second")
        self.assertIn("Invalidated: cma, leaf, middle", notes)
        self.assertIn("Fresh untouched", rust)
        self.assertIn("Compiling leaf", rust)
        self.assertIn("Compiling middle", rust)
        self.assertEqual(output, "second:new:7")

    def test_single_file_edit_keeps_workspace_libraries_fresh(self):
        self.build("first")
        path = self.root / "cma/src/main.rs"
        self.write("cma/src/main.rs", path.read_text().replace("{}:{}:{}", "changed:{}:{}:{}"))
        # A later Git archive gives every unchanged source a newer mtime.
        stamp = time.time() + 10
        for source in self.root.glob("*/src/*.rs"):
            os.utime(source, (stamp, stamp))
        notes, rust, output = self.build("second")
        self.assertIn("Invalidated: cma\n", notes)
        for name in ("leaf", "middle", "untouched"):
            self.assertIn(f"Fresh {name}", rust)
        self.assertEqual(output, "changed:second:old:7")

    def test_invalid_receipt_and_removed_path_force_cold_build(self):
        self.build("first")
        self.write("target/release/deps/foreign", "unexpected output")
        notes, rust, _ = self.build("second")
        self.assertIn("workspace output inventory differs", notes)
        self.assertIn("Compiling untouched", rust)
        (self.root / "leaf/src/value.txt").rename(self.root / "leaf/src/new.txt")
        self.write("leaf/src/lib.rs", 'pub fn value() -> &\'static str { include_str!("new.txt") }\n')
        notes, rust, output = self.build("third")
        self.assertIn("workspace path set changed", notes)
        self.assertIn("Compiling untouched", rust)
        self.assertEqual(output, "third:old:7")

    def test_identity_change_keeps_libraries_fresh(self):
        self.build("first")
        notes, rust, output = self.build("second")
        self.assertIn("Invalidated: none", notes)
        self.assertIn("Fresh leaf", rust)
        self.assertIn("Compiling cma", rust)
        self.assertEqual(output, "second:old:7")

    def test_old_timestamp_on_a_generated_input_rebuilds_its_dependents(self):
        script = self.write("leaf/build.rs", '''
fn main() {
    println!("cargo:rerun-if-changed=template.txt");
    let output = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    std::fs::copy("template.txt", output.join("value.txt")).unwrap();
}
''')
        self.write("leaf/template.txt", "generated-old")
        self.write("leaf/src/lib.rs",
                   'pub fn value() -> &\'static str { include_str!(concat!(env!("OUT_DIR"), "/value.txt")) }\n')
        with (patch.object(cache, "SCRIPT_INPUTS", ("leaf/build.rs",)),
              patch.object(cache, "SCRIPT_DIGESTS", (cache.digest(script),))):
            self.assertEqual(self.build("first")[2], "first:generated-old:7")
            self.write("leaf/template.txt", "generated-new")
            notes, rust, output = self.build("second")
            self.assertIn("Invalidated: cma, leaf, middle", notes)
            self.assertIn("Fresh untouched", rust)
            self.assertEqual(output, "second:generated-new:7")

    def test_working_outputs_never_keep_the_previous_receipt(self):
        self.build("first")
        saved = self.target / cache.SEED
        self.write("leaf/src/lib.rs", "this does not compile")
        cache.prepare(self.root, self.target, "cma", self.plan)
        self.assertFalse(saved.exists())
        with self.assertRaises(subprocess.CalledProcessError):
            self.command("cargo", "build", "--release", "--locked", "--offline", "--bin", "cma")
        self.assertFalse(saved.exists())


if __name__ == "__main__":
    unittest.main()
