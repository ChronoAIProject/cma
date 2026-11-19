"""The empty upstream ring exception cannot authorize private material."""

import hashlib
import io
import json
from pathlib import Path
import tarfile
import tempfile
import time
import unittest
from unittest.mock import patch

import runtime_image_hygiene as GATE
import runtime_image_hygiene_policy as POLICY


def layer(items):
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode="w") as tar:
        for path, data, kind in items:
            member = tarfile.TarInfo(path)
            if kind == "symlink":
                member.type, member.linkname = tarfile.SYMTYPE, data
                tar.addfile(member)
            elif kind == "hardlink":
                member.type, member.linkname = tarfile.LNKTYPE, data
                tar.addfile(member)
            elif kind == "directory":
                member.type = tarfile.DIRTYPE
                tar.addfile(member)
            else:
                member.size = len(data)
                tar.addfile(member, io.BytesIO(data))
    return output.getvalue()


def docker_image(path, layers, *, diffs=None):
    config = json.dumps({"os": "linux", "architecture": "amd64", "rootfs": {
        "type": "layers", "diff_ids": diffs or ["sha256:" + hashlib.sha256(v).hexdigest() for v in layers],
    }}).encode()
    config_name = hashlib.sha256(config).hexdigest() + ".json"
    names = [f"layer-{index}.tar" for index in range(len(layers))]
    manifest = json.dumps([{"Config": config_name, "Layers": names}]).encode()
    with tarfile.open(path, "w") as tar:
        for name, value in [("manifest.json", manifest), (config_name, config), *zip(names, layers)]:
            member = tarfile.TarInfo(name)
            member.size = len(value)
            tar.addfile(member, io.BytesIO(value))


def final_entries():
    return [("etc/machine-id", b"", "file"),
            ("var/lib/dbus/machine-id", "/etc/machine-id", "symlink")]


class RuntimeHygieneTests(unittest.TestCase):
    def test_default_image_keeps_the_browser_without_its_vendor_credential_hook(self):
        for kind in ("file", "symlink", "hardlink"):
            with self.subTest(kind=kind), self.assertRaisesRegex(POLICY.HygieneRefusal, "browser_credential_hook"):
                POLICY.layer_entry(None, 1, "etc/chromium.d/apikeys", kind, 1, component="runtime")
        self.assertFalse(POLICY.layer_entry(None, 1, "usr/lib/chromium/chromium", "file", 1, component="runtime"))

    def test_vendor_examples_are_excluded_without_removing_modules_or_licenses(self):
        root = "usr/local/lib/node_modules/"
        npm = root + "npm/node_modules/"
        zod = root + "@agentclientprotocol/codex-acp/node_modules/zod/src/v4/"
        for path in ("npm/docs/content/using-npm/config.md", "npm/docs/output/using-npm/config.html",
                     "npm/man/man7/config.7"):
            with self.subTest(path=path), self.assertRaisesRegex(POLICY.HygieneRefusal, "vendor_examples_layer"):
                POLICY.layer_entry(None, 1, root + path, "file", 1)
        for path in ("npm/lib/commands/config.js", "npm/LICENSE"):
            self.assertFalse(POLICY.layer_entry(None, 1, root + path, "file", 1))
        for package in ("@npmcli/arborist", "ip-address"):
            with self.subTest(package=package):
                with self.assertRaisesRegex(POLICY.HygieneRefusal, "vendor_examples_layer"):
                    POLICY.layer_entry(None, 1, npm + package + "/README.md", "file", 1)
                self.assertFalse(POLICY.layer_entry(None, 1, npm + package + "/LICENSE", "file", 1))
        for variant in ("classic", "mini"):
            with self.subTest(variant=variant):
                with self.assertRaisesRegex(POLICY.HygieneRefusal, "vendor_examples_layer"):
                    POLICY.layer_entry(None, 1, zod + variant + "/tests/example.ts", "file", 1)
                self.assertFalse(POLICY.layer_entry(None, 1, zod + variant + "/index.ts", "file", 1))

    def test_private_ipc_identity_is_exact_and_must_be_uninitialized_in_every_layer(self):
        path, target = "etc/machine-id", POLICY.IPC_ID_TARGET
        state = {path: {"kind": "symlink", "target": target},
                 "var/lib/dbus/machine-id": {"kind": "symlink", "target": "/etc/machine-id"}}
        POLICY.final_filesystem(state, "runtime")
        for component, destination in (("runtime-omarchy", target), ("runtime", target + "-other")):
            with self.assertRaises(POLICY.HygieneRefusal):
                POLICY.layer_entry(None, 1, path, "symlink", 0, target=destination, component=component)
            with self.assertRaises(POLICY.HygieneRefusal):
                POLICY.final_filesystem(dict(state, **{path: {"kind": "symlink", "target": destination}}), component)
        for location, kind in ((POLICY.IPC_ID_DIR, "directory"), (target.lstrip("/"), "file"),
                               ("dev/shm", "symlink"), ("dev", "hardlink")):
            with self.assertRaises(POLICY.HygieneRefusal):
                POLICY.layer_entry(None, 1, location, kind, 0, component="runtime")

    def test_archive_checks_every_layer_before_accepting_the_sandbox_identity_link(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            archive = root / "fixture.tar"
            initialized = layer([("etc/machine-id", POLICY.IPC_ID_TARGET, "symlink")])
            lower = layer(final_entries())
            docker_image(archive, [lower, initialized])
            receipt = GATE.check_archive(archive, "runtime", root / "good.json")
            self.assertEqual(receipt["machine_identity"], "uninitialized")
            with self.assertRaisesRegex(POLICY.HygieneRefusal, "machine_identity_layer"):
                GATE.check_archive(archive, "runtime-omarchy", root / "wrong-component.json")
            tainted = layer([(POLICY.IPC_ID_DIR + "/machine-id", b"synthetic", "file")])
            docker_image(archive, [lower, tainted, initialized])
            with self.assertRaisesRegex(POLICY.HygieneRefusal, "machine_identity_layer"):
                GATE.check_archive(archive, "runtime", root / "tainted.json")

    def test_exception_matches_every_identity_field(self):
        values = [POLICY.ARCH_BASE, 0, POLICY.EMPTY_RING, "file", 0, POLICY.EMPTY_SHA256]
        self.assertTrue(POLICY.empty_ring_exception(*values))
        for index, changed in enumerate(("sha256:" + "f" * 64, 1, "root/secring.gpg",
                                         "hardlink", 1, "0" * 64)):
            candidate = values.copy()
            candidate[index] = changed
            self.assertFalse(POLICY.empty_ring_exception(*candidate))

    def test_base_identity_uses_complete_verified_prefix_and_component(self):
        self.assertEqual(POLICY.base_identity("runtime-omarchy", list(POLICY.ARCH_BASE_LAYERS)), POLICY.ARCH_BASE)
        for component, diffs in (("runtime", POLICY.ARCH_BASE_LAYERS),
                                 ("runtime-omarchy", POLICY.ARCH_BASE_LAYERS[:1]),
                                 ("runtime-omarchy", ("sha256:" + "f" * 64, POLICY.ARCH_BASE_LAYERS[1])),
                                 ("runtime-omarchy", (POLICY.ARCH_BASE_LAYERS[0], "sha256:" + "f" * 64))):
            self.assertIsNone(POLICY.base_identity(component, diffs))

    def test_any_nonempty_or_nonregular_private_storage_is_rejected(self):
        for path in (POLICY.EMPTY_RING, "home/cma/.gnupg/private-keys-v1.d/key",
                     "root/secring", "root/secring.gpg.backup"):
            for kind, size in (("file", 1), ("symlink", 0), ("hardlink", 0)):
                with self.subTest(path=path, kind=kind), self.assertRaises(POLICY.HygieneRefusal):
                    POLICY.layer_entry(POLICY.ARCH_BASE, 0, path, kind, size, POLICY.EMPTY_SHA256)
        with self.assertRaises(POLICY.HygieneRefusal):
            POLICY.layer_entry(POLICY.ARCH_BASE, 1, POLICY.EMPTY_RING, "file", 0, POLICY.EMPTY_SHA256)

    def test_opaque_and_whiteout_do_not_hide_same_layer_entries(self):
        entry = {"kind": "file", "size": 0}
        for opaque, whiteouts in (({"etc/pacman.d/gnupg"}, set()), (set(), {POLICY.EMPTY_RING})):
            state = {"old": entry}
            POLICY.apply_layer(state, {POLICY.EMPTY_RING: entry}, whiteouts, opaque)
            with self.assertRaisesRegex(POLICY.HygieneRefusal, "private_keyring_final"):
                POLICY.final_filesystem(state)

    def test_lower_private_path_must_be_removed_from_final_filesystem(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            first = layer([(POLICY.EMPTY_RING, b"", "file")])
            second = layer([("base-marker", b"public", "file")])
            approved = tuple("sha256:" + hashlib.sha256(v).hexdigest() for v in (first, second))
            archive = root / "image.tar"
            for cleanup in ([], [("etc/pacman.d/gnupg/.wh.secring.gpg", b"", "file")],
                            [("etc/pacman.d/gnupg/.wh..wh..opq", b"", "file")]):
                docker_image(archive, [first, second, layer(cleanup + final_entries())])
                output = root / ("receipt-" + str(len(list(root.iterdir()))) + ".json")
                with patch.object(POLICY, "ARCH_BASE_LAYERS", approved):
                    if not cleanup:
                        with self.assertRaisesRegex(POLICY.HygieneRefusal, "private_keyring_final"):
                            GATE.check_archive(archive, "runtime-omarchy", output)
                        self.assertFalse(output.exists())
                    else:
                        receipt = GATE.check_archive(archive, "runtime-omarchy", output)
                        self.assertEqual(receipt["status"], "PASS")
                        self.assertEqual(receipt["exceptions"], [{"base_digest": POLICY.ARCH_BASE,
                            "layer": 0, "path": POLICY.EMPTY_RING, "size": 0,
                            "content_sha256": POLICY.EMPTY_SHA256}])

    def test_unknown_base_or_forged_diff_id_cannot_claim_exception(self):
        GATE.readers()
        from image_secrets.common import ScanError
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            archive = root / "image.tar"
            first = layer([(POLICY.EMPTY_RING, b"", "file")])
            cleanup = layer([("etc/pacman.d/gnupg/.wh.secring.gpg", b"", "file")] + final_entries())
            for diffs in (None, list(POLICY.ARCH_BASE_LAYERS)):
                docker_image(archive, [first, cleanup], diffs=diffs)
                with self.assertRaises((POLICY.HygieneRefusal, ScanError)):
                    GATE.check_archive(archive, "runtime-omarchy", root / "report.json")
                self.assertFalse((root / "report.json").exists())

    def test_private_links_and_empty_final_directory_cannot_evade_inventory(self):
        cases = [(POLICY.EMPTY_RING, "elsewhere", "symlink"),
                 (POLICY.EMPTY_RING, "public-file", "hardlink"),
                 ("root/.gnupg/private-keys-v1.d", b"", "directory")]
        for entry in cases:
            with self.subTest(kind=entry[2]), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                docker_image(root / "image.tar", [layer([("public-file", b"", "file"), entry] + final_entries())])
                with self.assertRaises(POLICY.HygieneRefusal):
                    GATE.check_archive(root / "image.tar", "runtime-omarchy", root / "report.json")
                self.assertFalse((root / "report.json").exists())

    def test_replacing_an_ancestor_removes_only_covered_lower_paths(self):
        state = {POLICY.EMPTY_RING: {"kind": "file", "size": 0},
                 "etc/machine-id": {"kind": "file", "size": 0},
                 "var/lib/dbus/machine-id": {"kind": "symlink", "target": "/etc/machine-id"}}
        POLICY.apply_layer(state, {POLICY.PACMAN_RING: {"kind": "symlink", "target": "/public"}}, set(), set())
        self.assertNotIn(POLICY.EMPTY_RING, state)
        with self.assertRaisesRegex(POLICY.HygieneRefusal, "initialized_pacman_keyring"):
            POLICY.final_filesystem(state)
        POLICY.apply_layer(state, {}, {POLICY.PACMAN_RING}, set())
        self.assertEqual(set(state), POLICY.MACHINE_IDS)
        POLICY.final_filesystem(state)

    def test_build_local_public_keyring_must_be_removed_but_distro_inputs_ship(self):
        public = POLICY.PACMAN_RING + "/pubring.gpg"
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for cleanup in ([], [("etc/pacman.d/.wh.gnupg", b"", "file")]):
                archive, report = root / "image.tar", root / "report.json"
                docker_image(archive, [layer([(public, b"fixture", "file"),
                    ("usr/share/pacman/keyrings/example.gpg", b"fixture", "file")]),
                    layer(cleanup + final_entries())])
                if cleanup:
                    self.assertEqual(GATE.check_archive(archive, "runtime-omarchy", report)["status"], "PASS")
                else:
                    with self.assertRaisesRegex(POLICY.HygieneRefusal, "initialized_pacman_keyring"):
                        GATE.check_archive(archive, "runtime-omarchy", report)
                    self.assertFalse(report.exists())

    def test_sandbox_tmpfs_never_supplies_image_entries_and_ancestors_cannot_redirect_trust(self):
        with self.assertRaisesRegex(POLICY.HygieneRefusal, "sandbox_keyring_layer"):
            POLICY.layer_entry(None, 1, POLICY.SANDBOX_PACMAN + "/gnupg/pubring.gpg", "file", 1)
        for parent in POLICY.PACMAN_DIRECTORIES:
            state = {parent: {"kind": "symlink", "target": "/outside"}}
            with self.assertRaisesRegex(POLICY.HygieneRefusal, "initialized_pacman_keyring"):
                POLICY.final_filesystem(state)

    def test_deleted_private_material_and_machine_id_still_refuse(self):
        for path in (POLICY.EMPTY_RING, "etc/machine-id", "var/lib/pacman/sync/core.db"):
            with self.subTest(path=path), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                parts = path.rsplit("/", 1)
                whiteout = parts[0] + "/.wh." + parts[1]
                docker_image(root / "image.tar", [layer([(path, b"synthetic", "file")]),
                             layer([(whiteout, b"", "file")] + final_entries())])
                with self.assertRaises(POLICY.HygieneRefusal):
                    GATE.check_archive(root / "image.tar", "runtime", root / "report.json")
                self.assertFalse((root / "report.json").exists())

    def test_clean_image_accepts_escaped_posix_names_and_binds_archive(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            docker_image(root / "image.tar", [layer(final_entries() + [(r"usr/lib/systemd/test\x2dunit", b"public", "file")])])
            receipt = GATE.check_archive(root / "image.tar", "runtime", root / "report.json")
            self.assertEqual(receipt["archive_sha256"], hashlib.sha256((root / "image.tar").read_bytes()).hexdigest())
            self.assertEqual(receipt["exceptions"], [])
            with self.assertRaisesRegex(POLICY.HygieneRefusal, "output_exists"):
                GATE.check_archive(root / "image.tar", "runtime", root / "report.json")

    def test_appended_or_unterminated_tar_payloads_refuse(self):
        GATE.readers()
        from image_secrets.common import ScanError
        clean = layer(final_entries())
        hidden = layer([("root/.gnupg/private-keys-v1.d/fixture", b"synthetic", "file")])
        with tarfile.open(fileobj=io.BytesIO(clean)) as tar:
            list(tar)
            end = tar.offset
        for boundary in ("layer", "outer"):
            for trailer in ("appended", "unterminated"):
                with self.subTest(boundary=boundary, trailer=trailer), tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    archive = root / "image.tar"
                    payload = clean + hidden if trailer == "appended" else clean[:end]
                    docker_image(archive, [payload if boundary == "layer" else clean])
                    if boundary == "outer":
                        original = archive.read_bytes()
                        with tarfile.open(archive) as tar:
                            list(tar)
                            outer_end = tar.offset
                        archive.write_bytes(original + hidden if trailer == "appended" else original[:outer_end])
                    with self.assertRaisesRegex((POLICY.HygieneRefusal, ScanError), "tar[_-]"):
                        GATE.check_archive(archive, "runtime", root / "report.json")
                    self.assertFalse((root / "report.json").exists())

    def test_duplicate_unsafe_or_special_entries_fail_closed(self):
        for items in ([('same', b'', 'file'), ('./same', b'', 'file')],
                      [('a/../b', b'', 'file')], [('/absolute', b'', 'file')],
                      [('etc/.wh.machine-id', b'nonempty', 'file')]):
            with self.subTest(items=items), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                docker_image(root / "image.tar", [layer(items + final_entries())])
                with self.assertRaises(POLICY.HygieneRefusal):
                    GATE.check_archive(root / "image.tar", "runtime", root / "report.json")
        with self.assertRaises(POLICY.HygieneRefusal):
            POLICY.layer_entry(None, 0, "node", "device", 0)
        special = tarfile.TarInfo("device")
        special.type = tarfile.CHRTYPE
        with self.assertRaises(POLICY.HygieneRefusal):
            GATE.member_kind(special)

    def test_timeout_and_incomplete_inventory_never_produce_receipt(self):
        _, Budget, _ = GATE.readers()
        with tarfile.open(fileobj=io.BytesIO(layer(final_entries()))) as tar:
            with self.assertRaisesRegex(POLICY.HygieneRefusal, "inventory_timeout"):
                GATE.inventory_layer(tar, {}, None, 0, Budget(4096, 10), time.monotonic() - 1)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            docker_image(root / "image.tar", [layer(final_entries())])
            with patch.object(GATE, "readers", side_effect=OSError("sensitive diagnostic")), \
                    patch("builtins.print") as printed:
                self.assertEqual(GATE.main(["--archive", str(root / "image.tar"), "--component", "runtime",
                                           "--output", str(root / "report.json")]), 1)
            self.assertEqual(printed.call_args.args, ("Runtime layer hygiene refused: incomplete_inventory",))
            self.assertFalse((root / "report.json").exists())


if __name__ == "__main__":
    unittest.main()
