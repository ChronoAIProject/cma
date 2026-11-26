#!/usr/bin/env python3
"""Build a real workspace to expose stale artifacts and unnecessary recompiles."""

from contextlib import redirect_stdout
import importlib.util
from io import StringIO
import json
import os
from pathlib import Path
import re
import shutil
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

    def build(self, stamp, chef=None):
        os.environ["CHRONO_BUILD_SHA"] = stamp
        with redirect_stdout(StringIO()) as notes:
            cache.prepare(self.root, self.target, "cma", self.plan)
        if chef is not None:
            cache.hydrate_dependencies(chef, self.target)
        invocation = self.command("cargo", "build", "--release", "--locked", "--offline", "--verbose", "--bin", "cma")
        output = self.command(str(self.target / "release" / "cma")).stdout.strip()
        cache.finish(self.target, self.plan, "cma")
        return notes.getvalue(), invocation.stderr, output

    def test_chef_objects_reach_overlapping_target_directories(self):
        chef = self.root / "chef"
        dependency = self.write("chef/release/deps/libregistry.rlib", "dependency")
        self.write("chef/release/.fingerprint/registry-1234/lib-registry", "fingerprint")
        self.write("chef/release/deps/libworkspace.rlib", "placeholder")
        workspace_object = self.write("target/release/deps/libworkspace.rlib", "verified")
        (self.target / "release/.fingerprint").mkdir()
        cache.hydrate_dependencies(chef, self.target)
        installed = self.target / "release/deps/libregistry.rlib"
        self.assertEqual(installed.read_text(), "dependency")
        self.assertEqual(installed.stat().st_mtime_ns, dependency.stat().st_mtime_ns)
        self.assertEqual(workspace_object.read_text(), "verified")
        self.assertEqual((self.target / "release/.fingerprint/registry-1234/lib-registry").read_text(),
                         "fingerprint")
        self.write("chef/release/deps/new-directory/item", "dependency")
        obstruction = self.write("target/release/deps/new-directory", "occupied")
        with self.assertRaisesRegex(ValueError, "Chef directory conflicts"):
            cache.hydrate_dependencies(chef, self.target)
        self.assertEqual(obstruction.read_text(), "occupied")

    def test_trim_keeps_the_latest_finalized_session_with_its_rustc_lock(self):
        relative = "target/release/incremental/cma-abc"
        directory = self.root / relative
        self.write(relative + "/s-1-abc-old/work-products.bin", "old")
        self.write(relative + "/s-1-abc.lock", "")
        current = self.write(relative + "/s-2-def-final/work-products.bin", "ready")
        os.utime(current, (200, 200))
        lock = self.write(relative + "/s-2-def.lock", "")
        missing = self.write(relative + "/s-3-ghi-final/work-products.bin", "lock missing")
        os.utime(missing, (250, 250))
        partial = self.write(relative + "/s-4-jkl-working/work-products.bin", "partial")
        os.utime(partial, (300, 300))
        self.write(relative + "/s-4-jkl.lock", "")
        self.write(relative + "/s-5-mno.lock", "")
        cache.trim(self.target, {"cma"}, "cma")
        self.assertEqual(set(directory.iterdir()), {current.parent, lock})

    def test_archive_timestamp_does_not_hide_transitive_input_change(self):
        _, _, seeded = self.build("first")
        self.assertEqual(seeded, "first:old:7")
        self.write("leaf/src/value.txt", "new")
        notes, rust, output = self.build("second")
        self.assertIn("unqualified workspace input change", notes)
        self.assertIn("Compiling untouched", rust)
        self.assertIn("Compiling leaf", rust)
        self.assertIn("Compiling middle", rust)
        self.assertEqual(output, "second:new:7")

    def test_identical_source_keeps_workspace_libraries_fresh(self):
        self.build("first")
        # A later archive can give unchanged files a newer mtime.
        stamp = time.time() + 10
        for source in self.root.glob("*/src/*.rs"):
            os.utime(source, (stamp, stamp))
        notes, rust, output = self.build("first")
        self.assertIn("Invalidated: none", notes)
        for name in ("leaf", "middle", "untouched"):
            self.assertIn(f"Fresh {name}", rust)
        self.assertEqual(output, "first:old:7")

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

    def test_unqualified_identity_change_clears_the_workspace_outputs(self):
        self.build("first")
        notes, rust, output = self.build("second")
        self.assertIn("unqualified workspace input change", notes)
        self.assertIn("Compiling leaf", rust)
        self.assertIn("Compiling cma", rust)
        self.assertEqual(output, "second:old:7")

    def split_targets(self):
        (self.root / "crates").mkdir()
        (self.root / "cma").rename(self.root / "crates/cma")
        manifest = self.root / "crates/cma/Cargo.toml"
        self.write("crates/cma/Cargo.toml", manifest.read_text().replace('"../', '"../../'))
        root_manifest = self.root / "Cargo.toml"
        self.write("Cargo.toml", root_manifest.read_text().replace('"cma"', '"crates/cma"'))
        main = "crates/cma/src/main.rs"
        self.write(main,
                   'const VALUE: u32 = 41;\nfn main() { println!("{}:{}:{}", '
                   'env!("CHRONO_BUILD_SHA"), VALUE, cma::value()); }\n')
        self.write("crates/cma/src/lib.rs", "pub fn value() -> u32 { 41 }\n")
        return main

    def vendored_reader(self, reader, owner="crates/cma"):
        # This is a registry proc macro in Cargo's resolved graph. Marking it
        # as a workspace macro would disable reuse before reaching the guard.
        vendor = self.root / "vendor"
        package = vendor / "fixture-reader-1.0.0"
        self.write("vendor/fixture-reader-1.0.0/Cargo.toml",
                   '[package]\nname="fixture-reader"\nversion="1.0.0"\nedition="2021"\n'
                   '[lib]\nproc-macro=true\n')
        self.write("vendor/fixture-reader-1.0.0/src/lib.rs", """
#[proc_macro]
pub fn snapshot(_: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let root = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let value = """ + reader + """;
    value.to_string().parse().unwrap()
}
""")
        checksums = {path.relative_to(package).as_posix(): cache.digest(path)
                     for path in package.rglob("*") if path.is_file()}
        self.write("vendor/fixture-reader-1.0.0/.cargo-checksum.json",
                   json.dumps({"files": checksums, "package": None}))
        self.write(".cargo/config.toml",
                   '[source.crates-io]\nreplace-with="fixture"\n[source.fixture]\n'
                   f'directory="{vendor}"\n')
        manifest = f"{owner}/Cargo.toml"
        self.write(manifest, (self.root / manifest).read_text() + 'fixture-reader = "1.0.0"\n')
        self.command("cargo", "generate-lockfile", "--offline")

    def approved_projection(self, main):
        # Exercise the real source gate on portable runners. Only its known
        # compiler identity and the review's pinned projection are fixtures.
        toolchain = cache.tool_output

        def native_tool(*command):
            if command == ("rustc", "-vV"):
                return ("host: x86_64-unknown-linux-musl\n"
                        "commit-hash: 31fca3adb283cc9dfd56b49cdee9a96eb9c96ffd\n")
            return toolchain(*command)

        packages = ("cma",)
        files = cache.inventory(self.root, skip=("target",))
        entrypoint = cache.audit_digest(files, packages, excluded=lambda path: path == main)
        dependencies = cache.audit_digest(
            files, packages, excluded=lambda path: path.startswith("crates/cma/src/") and path.endswith(".rs"),
        )
        self.enterContext(patch.dict(cache.RELEASE_AUDITS, {"cma": (packages, entrypoint, dependencies)}))
        self.enterContext(patch.object(cache, "tool_output", native_tool))

    def test_chef_layer_cannot_restore_workspace_fingerprints(self):
        main = self.split_targets()
        self.approved_projection(main)
        library = "crates/cma/src/lib.rs"
        with tempfile.TemporaryDirectory() as temporary:
            chef = Path(temporary) / "chef"
            self.write(library, "pub fn value() -> u32 { 99 }")
            self.command("cargo", "build", "--release", "--locked", "--offline", "--bin", "cma")
            for binary in ("cma", "cmaeg"):
                recipe = (ROOT / f"api/crates/{binary}/Dockerfile").read_text()
                dependencies = recipe.split("FROM chef AS dependencies\n", 1)[1].split("\nFROM ", 1)[0]
                self.assertRegex(dependencies, r"cargo chef cook [^\n]+\\\n"
                                               r"    && cargo clean --release --workspace --locked")
            self.command("cargo", "clean", "--release", "--workspace", "--locked")
            self.target.rename(chef)
            self.target.mkdir()
            self.write(library, "pub fn value() -> u32 { 41 }")
            self.assertEqual(self.build("first", chef)[2], "first:41:41")
            self.write(library, "pub fn value() -> u32 { 42 }")
            self.assertEqual(self.build("second", chef)[2], "second:41:42")
            notes, rust, output = self.build("third", chef)
            self.assertIn("Invalidated: cma", notes)
            self.assertIn("Fresh untouched", rust)
            self.assertEqual(output, "third:41:42")

    def test_audited_stamp_and_executable_edit_retain_same_package_library(self):
        main = self.split_targets()
        self.approved_projection(main)
        self.assertEqual(self.build("first")[2], "first:41:41")
        objects = {path.name: path.stat().st_mtime_ns
                   for path in (self.target / "release/deps").glob("libcma-*.rlib")}
        self.assertTrue(objects)
        notes, rust, output = self.build("second")
        self.assertIn("retained cma library under its release audit", notes)
        self.assertNotIn("crates/cma/src/lib.rs --error-format", rust)
        self.assertEqual(output, "second:41:41")
        self.write(main, (self.root / main).read_text().replace("= 41", "= 42"))
        notes, rust, output = self.build("third")
        self.assertIn("retained cma library under its release audit", notes)
        for name in ("leaf", "middle", "untouched"):
            self.assertIn(f"Fresh {name}", rust)
        self.assertNotIn("crates/cma/src/lib.rs --error-format", rust)
        self.assertEqual(output, "third:42:41")
        self.assertEqual({path.name: path.stat().st_mtime_ns
                          for path in (self.target / "release/deps").glob("libcma-*.rlib")}, objects)
        self.assertEqual(self.clean_output("third"), output)

    def clean_output(self, stamp):
        self.command("cargo", "clean", "--workspace", "--locked", "--release")
        os.environ["CHRONO_BUILD_SHA"] = stamp
        self.command("cargo", "build", "--release", "--locked", "--offline", "--bin", "cma")
        return self.command(str(self.target / "release/cma")).stdout.strip()

    def test_opaque_reader_overlapping_rustc_dep_info_uses_clean_workspace(self):
        main = self.split_targets()
        self.vendored_reader('if std::fs::read_to_string(root.join("src/value.rs")).unwrap()'
                             '.contains("42") { 42 } else { 41 }')
        self.write(main, (self.root / main).read_text().replace(
            "const VALUE: u32 = 41;", 'include!("value.rs");'))
        self.write("crates/cma/src/lib.rs", "pub fn value() -> u32 { fixture_reader::snapshot!() }")
        self.write("crates/cma/src/value.rs", "const VALUE: u32 = 41;")
        self.assertEqual(self.build("first")[2], "first:41:41")
        self.write("crates/cma/src/value.rs", "const VALUE: u32 = 42;")
        notes, _, output = self.build("second")
        self.assertIn("unqualified workspace input change", notes)
        self.assertEqual(output, "second:42:42")
        self.assertEqual(self.clean_output("second"), output)

    def test_isolated_stamp_keeps_the_edited_library_fresh_on_the_next_release(self):
        main = self.split_targets()
        wrapper = self.root / "build_support/release_rustc.py"
        wrapper.parent.mkdir()
        shutil.copy2(ROOT / "api/build_support/release_rustc.py", wrapper)
        self.enterContext(patch.dict(os.environ, {"RUSTC_WRAPPER": str(wrapper)}))
        self.vendored_reader('if std::env::var_os("CHRONO_BUILD_SHA").is_none() { 42 } else { 99 }')
        self.approved_projection(main)
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        chef = Path(temporary.name) / "chef"
        self.command("cargo", "build", "--release", "--locked", "--offline", "--bin", "cma")
        self.command("cargo", "clean", "--release", "--workspace", "--locked")
        self.target.rename(chef)
        self.target.mkdir()
        self.assertEqual(self.build("first", chef)[2], "first:41:41")
        incremental = self.target / "release/incremental"
        retained = set(incremental.rglob("work-products.bin"))
        self.assertTrue(retained)

        self.write("crates/cma/src/lib.rs", "pub fn value() -> u32 { fixture_reader::snapshot!() }")
        original = cache.invalidate_service
        def inspect_invalidation(*args, **kwargs):
            original(*args, **kwargs)
            self.assertEqual(set(incremental.rglob("work-products.bin")), retained)

        with patch.object(cache, "invalidate_service", inspect_invalidation), redirect_stdout(StringIO()) as reuse:
            notes, rust, output = self.build("second", chef)
        self.assertIn("Invalidated: cma", notes)
        self.assertIn("crates/cma/src/lib.rs --error-format", rust)
        self.assertEqual(output, "second:41:42")
        objects = re.search(r"incremental objects: restored=(\d+) reused=(\d+)", reuse.getvalue())
        self.assertIsNotNone(objects, reuse.getvalue())
        self.assertGreater(int(objects[2]), 0)

        notes, rust, output = self.build("third", chef)
        self.assertIn("retained cma library under its release audit", notes)
        self.assertNotIn("crates/cma/src/lib.rs --error-format", rust)
        self.assertEqual(output, "third:41:42")

    def test_edited_library_is_rebuilt_for_the_next_release_from_its_seed(self):
        main = self.split_targets()
        self.vendored_reader('if std::env::var("CHRONO_BUILD_SHA").unwrap() == "first" { 41 } else { 42 }')
        self.approved_projection(main)
        self.assertEqual(self.build("first")[2], "first:41:41")
        lockfile = cache.digest(self.root / "Cargo.lock")
        self.write("crates/cma/src/lib.rs", "pub fn value() -> u32 { fixture_reader::snapshot!() }")
        notes, rust, output = self.build("first")
        self.assertIn("Invalidated: cma", notes)
        self.assertIn("Fresh untouched", rust)
        self.assertIn("crates/cma/src/lib.rs --error-format", rust)
        self.assertEqual(output, "first:41:41")

        # The fresh seed contains the edited library. A release-stamp change
        # still invalidates its opaque macro while equal dependencies stay fresh.
        notes, rust, output = self.build("second")
        self.assertIn("Invalidated: cma", notes)
        self.assertIn("Fresh untouched", rust)
        self.assertIn("crates/cma/src/lib.rs --error-format", rust)
        self.assertEqual(output, "second:41:42")
        self.assertEqual(self.clean_output("second"), output)
        self.assertEqual(cache.digest(self.root / "Cargo.lock"), lockfile)

    def test_transitive_macro_observes_new_stamp(self):
        self.verify_transitive_reader(
            'if std::env::var("CHRONO_BUILD_SHA").unwrap() == "second" { 42 } else { 41 }',
            edit_entrypoint=False,
        )

    def test_transitive_macro_observes_edited_service_entrypoint(self):
        self.verify_transitive_reader(
            'if std::fs::read_to_string(root.join("../crates/cma/src/main.rs")).unwrap()'
            '.contains("42") { 42 } else { 41 }',
            edit_entrypoint=True,
        )

    def verify_transitive_reader(self, reader, *, edit_entrypoint):
        main = self.split_targets()
        self.vendored_reader(reader, owner="leaf")
        self.write("leaf/src/lib.rs", "pub fn value() -> u32 { fixture_reader::snapshot!() }")
        self.write("middle/src/lib.rs", "pub fn value() -> u32 { leaf::value() }")
        self.write("crates/cma/src/lib.rs", "pub fn value() -> u32 { middle::value() }")
        staging = tempfile.TemporaryDirectory()
        self.addCleanup(staging.cleanup)
        chef = Path(staging.name) / "chef"
        self.command("cargo", "build", "--release", "--locked", "--offline", "--bin", "cma")
        self.command("cargo", "clean", "--workspace", "--locked", "--release")
        self.target.rename(chef)
        self.target.mkdir()
        self.assertEqual(self.build("first", chef)[2], "first:41:41")
        if edit_entrypoint:
            self.write(main, (self.root / main).read_text().replace("= 41", "= 42"))
        notes, rust, output = self.build("second", chef)
        self.assertIn("unqualified workspace input change", notes)
        self.assertIn("Compiling leaf", rust)
        self.assertEqual(output, "second:42:42" if edit_entrypoint else "second:41:42")
        self.assertEqual(self.clean_output("second"), output)

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
            self.assertIn("unqualified workspace input change", notes)
            self.assertIn("Compiling untouched", rust)
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


class ImageProofTests(unittest.TestCase):
    def test_library_proof_rejects_the_previous_release_stamp(self):
        benchmark_path = ROOT / ".github/scripts/workspace-image-benchmark.py"
        spec = importlib.util.spec_from_file_location("image_benchmark", benchmark_path)
        benchmark = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(benchmark)
        revision = "a" * 40
        for component, library_output in (
            ("am", {"schema": benchmark.LIBRARY_PROBE}),
            ("eg", {"info": {"title": benchmark.LIBRARY_PROBE}}),
        ):
            with self.subTest(component=component):
                for version in ("1.2.3", "1.2.4"):
                    def runner(args, **_):
                        if args[-1] == "--workspace-cache-probe":
                            output = f"workspace-probe:baseline:{revision}:{version}".encode()
                        else:
                            output = json.dumps(library_output).encode()
                        return subprocess.CompletedProcess(args, 0, output, b"")

                    with patch.object(benchmark, "run", runner):
                        if version == "1.2.3":
                            with self.assertRaisesRegex(ValueError, "expected probe and release identity"):
                                benchmark.verify_image(Path("fixture"), component, revision, "1.2.4", probe="library")
                        else:
                            benchmark.verify_image(Path("fixture"), component, revision, "1.2.4", probe="library")


if __name__ == "__main__":
    unittest.main()
