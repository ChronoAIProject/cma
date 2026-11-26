"""Native build identity, credential-free arguments and archive/source boundaries."""
import io
import hashlib
import json
import os
import re
import tarfile
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

import ci_retry
from staging_cd_build import build, command, identity, inspect, recipe, snapshot
from staging_cd_common import Refusal
from staging_cd_publication_diagnostics import ImageCommandRefusal
from staging_cd_receipts import canonical, IDENTITY_LABEL
from staging_cd_receipts_tests import fixture_identity
from staging_cd_receipts_tests import fixture_receipt
from staging_cd_contract import VERSION_FILE, RECIPE
from staging_cd_contract_tests import fixture_config
from staging_cd_reservations import next_reservation


class BuildTests(unittest.TestCase):
    def source(self, root):
        manifest = Path(__file__).with_name("deployables.json")
        path = root / ".github/scripts/deployables.json"
        path.parent.mkdir(parents=True)
        path.write_bytes(manifest.read_bytes())
        dockerfile = root / "web/Dockerfile"
        dockerfile.parent.mkdir()
        dockerfile.write_text("FROM scratch AS runtime\nARG VITE_OUTBOUND_SEPARATE_MESSAGE_ENABLED=false\n")
        value = json.loads(path.read_text())
        value["cma-web"]["dockerfile_sha256"] = hashlib.sha256(dockerfile.read_bytes()).hexdigest()
        path.write_text(json.dumps(value))
        (root / ".dockerignore").write_text(".env*\n*.pem\n*.key\n.git\n.npmrc\nkubeconfig\ncredentials\n")

    def test_image_identity_changes_when_version_recipe_or_tree_changes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.source(root)
            reservation = next_reservation("web", "a" * 40, "b" * 40, 42, "patch", 1, 1, [], [])
            first, _, args = identity(reservation, root)
            self.assertEqual(args["CHRONO_BUILD"], "0.10.1")
            self.assertEqual(args["CHRONO_BUILD_SHA"], "a" * 40)
            self.assertEqual(args["VITE_OUTBOUND_SEPARATE_MESSAGE_ENABLED"], "true")
            self.assertEqual(args["OS_SECURITY_REFRESH"], "a" * 40, "non-runtime images keep a per-build refresh")
            (root / "web/Dockerfile").write_text("FROM scratch AS runtime\nUSER 101\n")
            with self.assertRaises(Refusal):
                identity(reservation, root)
            manifest = root / ".github/scripts/deployables.json"
            value = json.loads(manifest.read_text())
            value["cma-web"]["dockerfile_sha256"] = hashlib.sha256((root / "web/Dockerfile").read_bytes()).hexdigest()
            manifest.write_text(json.dumps(value))
            second, _, _ = identity(reservation, root)
            self.assertNotEqual(first["build"], second["build"])
            self.assertNotIn("VITE_OUTBOUND_SEPARATE_MESSAGE_ENABLED", identity(reservation, root)[2])
            (root / ".dockerignore").write_text("")
            with self.assertRaises(Refusal):
                recipe(root, "web")

    def test_build_once_uses_exact_version_and_never_receives_credentials(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            def materialize(_root, _revision, destination, _runner):
                destination.mkdir()
                self.source(destination)
            def runner(args, **_kwargs):
                archive = next(value.split("type=oci,dest=", 1)[1] for value in args if value.startswith("type=oci,dest="))
                Path(archive).write_bytes(b"synthetic image")
            run = Mock(side_effect=runner)
            reservation = next_reservation("web", "a" * 40, "b" * 40, 42, "patch", 1, 1, [], [])
            with patch("staging_cd_build.snapshot", side_effect=materialize):
                archive, release = build(root, reservation, root, run)
            self.assertTrue(archive.is_file())
            self.assertEqual(run.call_count, 1)
            args = run.call_args.args[0]
            self.assertIn("CHRONO_BUILD=0.10.1", args)
            self.assertIn("VITE_OUTBOUND_SEPARATE_MESSAGE_ENABLED=true", args)
            self.assertIn("--provenance=false", args)
            self.assertIn(IDENTITY_LABEL + "=" + canonical(release), args)
            with patch.dict(os.environ, {"GH_TOKEN": "fixture", "NYXID_CHRONO_CD_KEY": "fixture"}), \
                    patch("staging_cd_build.subprocess.run", return_value=Mock(returncode=0, stdout=b"done")) as child:
                self.assertEqual(command(["build-tool"]), b"done")
                self.assertNotIn("GH_TOKEN", child.call_args.kwargs["env"])
                self.assertNotIn("NYXID_CHRONO_CD_KEY", child.call_args.kwargs["env"])

    def test_failed_commands_mark_only_transport_failures_as_repeatable(self):
        for stderr, expected in (("read: connection reset by peer", ci_retry.TRANSIENT),
                                 ("received unexpected HTTP status: 503 Service Unavailable", ci_retry.TRANSIENT),
                                 ("denied: permission_denied: write_package", ci_retry.PERMANENT),
                                 ("Dockerfile parse error", ci_retry.PERMANENT)):
            with self.subTest(stderr=stderr), self.assertRaises(ImageCommandRefusal) as raised:
                command(["/bin/sh", "-c", 'printf "%s" "$0" >&2; exit 1', stderr], timeout=30)
            self.assertEqual(ci_retry.classify(raised.exception), expected)
            # The refusal keeps fixed text; the diagnostic is never part of it.
            self.assertNotIn(stderr, str(raised.exception))

    def test_source_archive_rejects_links_and_parent_traversal(self):
        for name, kind in (("../outside", tarfile.REGTYPE), ("link", tarfile.SYMTYPE)):
            with self.subTest(name=name), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                def runner(*_args, **_kwargs):
                    with tarfile.open(root / "source.tar", "w") as output:
                        member = tarfile.TarInfo(name)
                        member.type = kind
                        member.linkname = "../../outside"
                        output.addfile(member, io.BytesIO())
                with self.assertRaises(Refusal):
                    snapshot(root, "a" * 40, root / "source", runner)

    def runtime_source(self, root, component, pin="ARG DEBIAN_SNAPSHOT=20261121T100000Z\n"):
        self.source(root)
        project = Path(__file__).resolve().parents[2]
        for filename in (VERSION_FILE, RECIPE):
            destination = root / filename
            destination.parent.mkdir(parents=True)
            destination.write_bytes((project / filename).read_bytes())
        dockerfile = root / RECIPE
        unpinned = re.sub(r"(?m)^\s*ARG\s+DEBIAN_SNAPSHOT\s*=.*\n", "", dockerfile.read_text())
        dockerfile.write_text(pin + unpinned)
        manifest = root / ".github/scripts/deployables.json"
        values = json.loads(manifest.read_text())
        values["cma-" + component]["dockerfile_sha256"] = hashlib.sha256(dockerfile.read_bytes()).hexdigest()
        manifest.write_text(json.dumps(values))

    def test_runtime_security_refresh_follows_the_pinned_snapshot_not_the_source(self):
        for component in ("runtime", "runtime-omarchy"):
            with self.subTest(component=component), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                self.runtime_source(root, component)
                first = next_reservation(component, "a" * 40, "b" * 40, 42, "patch", 1, 1, [], [])
                second = next_reservation(component, "c" * 40, "d" * 40, 43, "patch", 1, 1, [], [])
                for reservation in (first, second):
                    _release, _value, arguments = identity(reservation, root)
                    self.assertEqual(arguments["OS_SECURITY_REFRESH"], "20261121T100000Z")
                    self.assertNotIn("VITE_OUTBOUND_SEPARATE_MESSAGE_ENABLED", arguments)

    def test_runtime_build_refuses_a_missing_or_ambiguous_snapshot_pin(self):
        pins = ("", "ARG DEBIAN_SNAPSHOT=latest\n",
                "ARG DEBIAN_SNAPSHOT=20261121T100000Z\nARG DEBIAN_SNAPSHOT=20261128T100000Z\n")
        for component in ("runtime", "runtime-omarchy"):
            for pin in pins:
                with self.subTest(component=component, pin=pin), tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    self.runtime_source(root, component, pin)
                    reservation = next_reservation(component, "a" * 40, "b" * 40, 42, "patch", 1, 1, [], [])
                    with self.assertRaisesRegex(Refusal, "DEBIAN_SNAPSHOT"):
                        identity(reservation, root)

    def test_runtime_exports_bind_fixed_epoch_and_snapshot_security_refresh(self):
        for component in ("runtime", "runtime-omarchy"):
            with self.subTest(component=component), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                self.runtime_source(root, component)
                reservation = next_reservation(component, "a" * 40, "b" * 40, 42, "patch", 1, 1, [], [])
                def runner(args, **_kwargs):
                    output = args[args.index("--output") + 1]
                    self.assertTrue(output.endswith(",rewrite-timestamp=true"))
                    self.assertIn("SOURCE_DATE_EPOCH=0", args)
                    self.assertIn("OS_SECURITY_REFRESH=20261121T100000Z", args)
                    self.assertNotIn("OS_SECURITY_REFRESH=" + reservation["source"], args)
                    self.assertNotIn("--squash", args)
                    Path(output.split("dest=", 1)[1].split(",")[0]).write_bytes(b"fixture")
                build(root, reservation, root, runner, source=root)

    def test_native_archive_requires_exact_identity_platform_and_unprivileged_user(self):
        expected = fixture_identity()
        manifest = b'{"schemaVersion":2,"config":{},"layers":[]}'
        config = {"architecture": "amd64", "os": "linux", "config": {"User": "101:101", "Labels": {
            "org.opencontainers.image.source": "https://github.com/ChronoAIProject/cma",
            IDENTITY_LABEL: canonical(expected), "org.opencontainers.image.revision": expected["source"],
            "org.opencontainers.image.version": expected["version"]}}}
        self.assertTrue(inspect(Path("fixture.tar"), expected, Mock(side_effect=[manifest, json.dumps(config).encode()])).startswith("sha256:"))
        for mutated in (dict(config, architecture="arm64"), dict(config, config=dict(config["config"], User="0")),
                        dict(config, config=dict(config["config"], Labels={})),
                        dict(config, config=dict(config["config"], Labels=None)), ["invalid"]):
            with self.assertRaises(Refusal):
                inspect(Path("fixture.tar"), expected, Mock(side_effect=[manifest, json.dumps(mutated).encode()]))

    def test_contract_labels_are_required_even_for_a_qualified_reused_image(self):
        receipt = fixture_receipt(component="am")
        expected, proof = receipt["identity"], receipt["compatibility"]
        manifest = b'{"schemaVersion":2,"config":{},"layers":[]}'
        config = fixture_config(receipt)
        config["config"]["User"] = "10001:10001"
        self.assertTrue(inspect(Path("fixture.tar"), expected,
            Mock(side_effect=[manifest, json.dumps(config).encode()]), compatibility=proof).startswith("sha256:"))
        for label in ("org.chronoai.cma.runtime-contract", "org.chronoai.cma.bootstrap",
                      "org.chronoai.cma.home-layout", IDENTITY_LABEL):
            modified = json.loads(json.dumps(config))
            modified["config"]["Labels"].pop(label)
            with self.subTest(label=label), self.assertRaises(Refusal):
                inspect(Path("fixture.tar"), expected, Mock(side_effect=[manifest, json.dumps(modified).encode()]),
                        compatibility=proof, allow_qualified=True)


if __name__ == "__main__":
    unittest.main()
