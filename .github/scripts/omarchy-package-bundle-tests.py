"""The Omarchy package bundle builder fails closed and writes reproducible bytes."""

import base64
import hashlib
import http.server
import importlib.util
import io
import json
import os
import shutil
import socket
import subprocess
import tarfile
import tempfile
import threading
import unittest
import unittest.mock
from pathlib import Path

SCRIPT = Path(__file__).with_name("omarchy-package-bundle.py")
SPEC = importlib.util.spec_from_file_location("omarchy_package_bundle", SCRIPT)
BUNDLE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BUNDLE)
GPG = shutil.which("gpg") and shutil.which("gpgv")


def digest(data):
    return hashlib.sha256(data).hexdigest()


class Server:
    """A local HTTP server whose handler is a function of (handler, request number)."""

    def __init__(self, respond, port=0):
        self.requests, self.respond = [], respond
        outer = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def do_GET(self):
                outer.requests.append((self.path, dict(self.headers)))
                outer.respond(self, len(outer.requests))

        self.httpd = http.server.ThreadingHTTPServer(("127.0.0.1", port), Handler)
        self.url = f"http://127.0.0.1:{self.httpd.server_address[1]}"
        self.thread = None

    def start(self):
        self.thread = threading.Thread(target=self.httpd.serve_forever, daemon=True)
        self.thread.start()

    def stop(self):
        if self.thread:
            self.httpd.shutdown()
        self.httpd.server_close()


def send(handler, status, body, headers=()):
    handler.send_response(status)
    for key, value in headers:
        handler.send_header(key, value)
    handler.send_header("Content-Length", str(len(body)))
    handler.end_headers()
    handler.wfile.write(body)


def ranged(payload):
    def respond(handler, _):
        start = int(handler.headers["Range"].split("=")[1].rstrip("-")) if handler.headers.get("Range") else 0
        status = 206 if start else 200
        send(handler, status, payload[start:], [("Content-Range", f"bytes {start}-{len(payload) - 1}/{len(payload)}")] if start else [])
    return respond


class LockTests(unittest.TestCase):
    def test_committed_lock_is_canonical_and_tagged_by_its_hash(self):
        lock, raw = BUNDLE.load()
        self.assertEqual(raw, BUNDLE.canonical(lock).encode())
        tag = BUNDLE.tag(lock, raw, lock["snapshot"])
        self.assertEqual(tag, lock["snapshot"].replace("/", "-") + "-" + digest(raw)[:12])
        with self.assertRaisesRegex(BUNDLE.BundleError, "does not match the lock"):
            BUNDLE.tag(lock, raw, "2026/10/23")
        kinds = {e["kind"] for e in lock["files"]}
        self.assertEqual(kinds, {"database", "package", "signature"})
        self.assertTrue(all(e["urls"][0].startswith("https://") for e in lock["files"]))

    def test_malformed_locks_are_refused(self):
        lock, raw = BUNDLE.load()
        cases = {
            "unsorted": lambda l: l["files"].reverse(),
            "unsafe path": lambda l: l["files"][0].update(path="../core.db"),
            "plain http": lambda l: l["files"][0]["urls"].__setitem__(0, l["files"][0]["urls"][0].replace("https", "http")),
            "other file": lambda l: l["files"][0]["urls"].__setitem__(0, l["files"][0]["urls"][0] + "x"),
            "bad digest": lambda l: l["files"][0].update(sha256="0" * 63),
            "extra field": lambda l: l["files"][0].update(note="x"),
            "missing signature": lambda l: l["files"].remove(next(e for e in l["files"] if e["kind"] == "signature")),
            "unknown keyring": lambda l: next(e for e in l["files"] if e["kind"] == "package").update(keyring="other"),
            "bad snapshot": lambda l: l.update(snapshot="2026-10-22"),
        }
        for name, mutate in cases.items():
            with self.subTest(name):
                changed = json.loads(raw)
                mutate(changed)
                with self.assertRaises(BUNDLE.BundleError):
                    BUNDLE.validate(changed)
        with self.assertRaisesRegex(BUNDLE.BundleError, "canonical"):
            BUNDLE.validate(lock, raw.replace(b'"files": [', b'"files":  ['))


class FetchTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.target = Path(self.directory.name) / "file.pkg.tar.zst"
        self.payload = os.urandom(300_000)

    def serve(self, respond, start=True, port=0):
        server = Server(respond, port)
        if start:
            server.start()
        self.addCleanup(server.stop)
        return server

    def test_sha256_mismatch_is_refused_and_nothing_is_kept(self):
        server = self.serve(lambda h, _: send(h, 200, b"x" * len(self.payload)))
        with self.assertRaisesRegex(BUNDLE.BundleError, "does not match the lock"):
            BUNDLE.fetch([server.url + "/file.pkg.tar.zst"], self.target, len(self.payload), digest(self.payload),
                         attempts=3, sleep=lambda _: None)
        self.assertFalse(self.target.exists())
        self.assertEqual(len(server.requests), 3)

    def test_reset_connection_resumes_from_the_partial_file(self):
        half = len(self.payload) // 2

        def respond(handler, number):
            if number == 1:
                handler.send_response(200)
                handler.send_header("Content-Length", str(len(self.payload)))
                handler.end_headers()
                handler.wfile.write(self.payload[:half])
                handler.wfile.flush()
                handler.connection.shutdown(2)
                return
            ranged(self.payload)(handler, number)

        server = self.serve(respond)
        sleeps = []
        BUNDLE.fetch([server.url + "/file.pkg.tar.zst"], self.target, len(self.payload), digest(self.payload),
                     attempts=3, sleep=sleeps.append)
        self.assertEqual(self.target.read_bytes(), self.payload)
        self.assertNotIn("Range", server.requests[0][1])
        self.assertEqual(server.requests[1][1].get("Range"), f"bytes={half}-")
        self.assertEqual(sleeps, [5])

    def test_refused_connection_is_retried_until_the_server_answers(self):
        with socket.socket() as probe:  # a free port with nothing listening: connections are refused
            probe.bind(("127.0.0.1", 0))
            port = probe.getsockname()[1]
        servers = []

        def start_server(_):
            if not servers:
                servers.append(self.serve(ranged(self.payload), port=port))

        BUNDLE.fetch([f"http://127.0.0.1:{port}/file.pkg.tar.zst"], self.target, len(self.payload),
                     digest(self.payload), attempts=4, sleep=start_server)
        self.assertEqual(self.target.read_bytes(), self.payload)
        self.assertEqual(len(servers[0].requests), 1)

    def test_every_failure_is_bounded_and_reported(self):
        server = self.serve(lambda h, _: send(h, 503, b"busy"))
        with self.assertRaisesRegex(BUNDLE.BundleError, "curl exit 22"):
            BUNDLE.fetch([server.url + "/file.pkg.tar.zst"], self.target, len(self.payload), digest(self.payload),
                         attempts=10, sleep=lambda _: None)
        self.assertEqual(len(server.requests), 10)

    def test_failing_host_is_tried_after_healthy_ones(self):
        hosts = BUNDLE.Hosts()
        urls = ["https://a.example/f", "https://b.example/f"]
        hosts.record(urls[0], True)
        self.assertEqual(hosts.order(urls), urls[::-1])
        hosts.record(urls[0], False)
        self.assertEqual(hosts.order(urls), urls)


def fake_package(files):
    """A zstd-compressed tar with the given members, as pacman packages are."""
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w") as t:
        for name, data in files.items():
            info = tarfile.TarInfo(name)
            info.size = len(data)
            t.addfile(info, io.BytesIO(data))
    return subprocess.run(["zstd", "-q", "-c"], input=buffer.getvalue(), capture_output=True, check=True).stdout


@unittest.skipUnless(GPG and shutil.which("zstd"), "gpg, gpgv and zstd are required")
class SignatureTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.home = self.root / "gnupg"
        self.home.mkdir(mode=0o700)
        self.keys = {name: self.key(name) for name in ("packager", "stranger")}

    def gpg(self, *args, data=None):
        return subprocess.run(["gpg", "--homedir", str(self.home), "--batch", "--quiet", "--pinentry-mode", "loopback",
                               "--passphrase", "", *args], input=data, capture_output=True, check=True).stdout

    def key(self, name):
        self.gpg("--quick-gen-key", f"{name} <{name}@example.invalid>", "ed25519", "sign", "never")
        listing = self.gpg("--with-colons", "--list-keys", f"{name}@example.invalid").decode()
        return next(line.split(":")[9] for line in listing.splitlines() if line.startswith("fpr"))

    def sign(self, name, data):
        return self.gpg("--local-user", self.keys[name], "--detach-sign", "--output", "-", data=data)

    def keyring(self, *names, armor=False):
        return self.gpg(*(["--armor"] if armor else []), "--export", *(self.keys[n] for n in names))

    def check(self, data, signature, revoked=()):
        package, sig, ring = self.root / "p.pkg.tar.zst", self.root / "p.pkg.tar.zst.sig", self.root / "ring.gpg"
        package.write_bytes(data)
        sig.write_bytes(signature)
        ring.write_bytes(self.keyring("packager"))
        work = self.root / "verify"
        work.mkdir(mode=0o700, exist_ok=True)
        BUNDLE.verify_signature(package, sig, ring, set(revoked), work)

    def test_good_signature_from_the_pinned_keyring_is_accepted(self):
        self.check(b"package", self.sign("packager", b"package"))

    def test_armored_keyring_dearmors_to_the_exported_packets(self):
        self.assertEqual(BUNDLE.dearmor(self.keyring("packager", armor=True)), self.keyring("packager"))
        self.assertEqual(BUNDLE.dearmor(self.keyring("packager")), self.keyring("packager"))
        with self.assertRaises(BUNDLE.BundleError):
            BUNDLE.dearmor(b"-----BEGIN PGP PUBLIC KEY BLOCK-----\n\nAAAA\n")

    def test_signature_refusals(self):
        cases = {"unknown signer": (b"package", self.sign("stranger", b"package"), ()),
                 "changed bytes": (b"packagE", self.sign("packager", b"package"), ()),
                 "revoked key": (b"package", self.sign("packager", b"package"), (self.keys["packager"],)),
                 "not a signature": (b"package", b"garbage", ())}
        for name, (data, signature, revoked) in cases.items():
            with self.subTest(name), self.assertRaises(BUNDLE.BundleError):
                self.check(data, signature, revoked)

    def database(self, entries):
        buffer = io.BytesIO()
        with tarfile.open(fileobj=buffer, mode="w:gz") as t:
            for filename, (name, data, signature) in entries.items():
                desc = (f"%FILENAME%\n{filename}\n\n%NAME%\n{name}\n\n%VERSION%\n1-1\n\n%CSIZE%\n{len(data)}\n\n"
                        f"%SHA256SUM%\n{digest(data)}\n\n%PGPSIG%\n{base64.b64encode(signature).decode()}\n\n")
                info = tarfile.TarInfo(f"{name}-1-1/desc")
                info.size = len(desc)
                t.addfile(info, io.BytesIO(desc.encode()))
        return buffer.getvalue()

    def tree(self, signed_by="packager"):
        """A minimal lock and bundle tree: one Arch package, both keyrings, one database."""
        # Arch ships its keyring ASCII-armored; gpgv needs it dearmored.
        rings = {"arch": fake_package({"usr/share/pacman/keyrings/archlinux.gpg": self.keyring("packager", armor=True),
                                       "usr/share/pacman/keyrings/archlinux-revoked": b""}),
                 "omarchy": fake_package({"usr/share/pacman/keyrings/omarchy.gpg": self.keyring("packager"),
                                          "usr/share/pacman/keyrings/omarchy-revoked": b""})}
        app = fake_package({"usr/bin/app": b"binary"})
        files = {"core/os/x86_64/archlinux-keyring-1-1-any.pkg.tar.zst": rings["arch"],
                 "core/os/x86_64/app-1-1-x86_64.pkg.tar.zst": app,
                 "omarchy/x86_64/omarchy-keyring-1-1-any.pkg.tar.zst": rings["omarchy"]}
        signatures = {path: self.sign(signed_by if "app-" in path else "packager", data) for path, data in files.items()}
        indexed = {Path(p).name: (Path(p).name.split("-1-1")[0], d, signatures[p]) for p, d in files.items() if p.startswith("core/")}
        files["core/os/x86_64/core.db"] = self.database(indexed)
        files.update({p + ".sig": s for p, s in signatures.items()})
        root = self.root / "bundle"
        entries = []
        for path, data in sorted(files.items()):
            (root / path).parent.mkdir(parents=True, exist_ok=True)
            (root / path).write_bytes(data)
            name = Path(path).name
            entry = {"path": path, "size": len(data), "sha256": digest(data), "urls": ["https://example.invalid/" + name]}
            if path.endswith(".db"):
                entry["kind"] = "database"
            elif path.endswith(".sig"):
                entry["kind"] = "signature"
            else:
                entry.update(kind="package", signature=path + ".sig", keyring="omarchy" if path.startswith("omarchy/") else "arch")
                if path.startswith("core/"):
                    entry["database"] = "core/os/x86_64/core.db"
            entries.append(entry)
        lock = {"schema": BUNDLE.SCHEMA, "snapshot": "2026/10/22", "files": entries,
                "keyrings": {"arch": "core/os/x86_64/archlinux-keyring-1-1-any.pkg.tar.zst",
                             "omarchy": "omarchy/x86_64/omarchy-keyring-1-1-any.pkg.tar.zst"}}
        return lock, root

    def test_verify_binds_packages_to_their_database_and_keyring(self):
        lock, root = self.tree()
        BUNDLE.verify(lock, root)
        changed = json.loads(json.dumps(lock))
        app = next(e for e in changed["files"] if "app-" in e["path"] and e["kind"] == "package")
        app["sha256"] = "1" * 64
        with self.assertRaisesRegex(BUNDLE.BundleError, "does not match its snapshot database"):
            BUNDLE.verify(changed, root)

    def test_verify_refuses_a_package_signed_outside_the_keyring(self):
        lock, root = self.tree(signed_by="stranger")
        with self.assertRaisesRegex(BUNDLE.BundleError, "signature is not a good signature"):
            BUNDLE.verify(lock, root)


class LayoutTests(unittest.TestCase):
    def test_layout_is_byte_reproducible_and_normalised(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "work"
            files = {"core/os/x86_64/core.db": b"db", "core/os/x86_64/a-1-1-any.pkg.tar.zst": b"a" * 5000,
                     "omarchy/x86_64/b-1-1-any.pkg.tar.zst": b"b"}
            for path, data in files.items():
                (root / path).parent.mkdir(parents=True, exist_ok=True)
                (root / path).write_bytes(data)
                os.utime(root / path, (12345, 67890))
            lock = {"schema": BUNDLE.SCHEMA, "snapshot": "2026/10/22", "keyrings": {},
                    "files": [{"path": p} for p in sorted(files)]}
            raw = b"lock bytes"
            results = []
            for run in ("one", "two"):
                layout = Path(directory) / run
                results.append(BUNDLE.write_layout(lock, raw, root, layout, "2026-10-22-abc"))
                os.utime(root / "core/os/x86_64/core.db", (1, 2))
            self.assertEqual(results[0], results[1])
            one = {p.relative_to(Path(directory) / "one"): p.read_bytes() for p in (Path(directory) / "one").rglob("*") if p.is_file()}
            two = {p.relative_to(Path(directory) / "two"): p.read_bytes() for p in (Path(directory) / "two").rglob("*") if p.is_file()}
            self.assertEqual(one, two)
            manifest = json.loads(one[Path("blobs/sha256") / results[0][0].split(":")[1]])
            self.assertEqual(manifest["annotations"]["org.opencontainers.image.source"], BUNDLE.SOURCE)
            self.assertEqual({layer["mediaType"] for layer in manifest["layers"]}, {BUNDLE.LAYER})
            names = []
            for layer in manifest["layers"]:
                with tarfile.open(fileobj=io.BytesIO(one[Path("blobs/sha256") / layer["digest"].split(":")[1]])) as t:
                    members = t.getmembers()
                self.assertEqual([m.name for m in members], sorted(m.name for m in members))
                self.assertTrue(all(m.mtime == 0 and m.uid == 0 and m.gid == 0 for m in members))
                names += [m.name for m in members if m.isfile()]
            self.assertEqual(sorted(names), sorted(["bundle/SNAPSHOT", "bundle/omarchy-packages.lock.json"]
                                                   + ["bundle/" + p for p in files]))
            index = json.loads(one[Path("index.json")])
            self.assertEqual(index["manifests"][0]["annotations"]["org.opencontainers.image.ref.name"], "2026-10-22-abc")


class AbsenceTests(unittest.TestCase):
    def check(self, status, body):
        def respond(handler, _):
            if handler.path.startswith("/token"):
                self.assertEqual(handler.headers["Authorization"], "Basic " + base64.b64encode(b"user:secret").decode())
                send(handler, 200, b'{"token": "bearer"}')
            else:
                self.assertEqual(handler.headers["Authorization"], "Bearer bearer")
                send(handler, status, body)

        server = Server(respond)
        server.start()
        self.addCleanup(server.stop)
        environment = {"REGISTRY_USER": "user", "REGISTRY_TOKEN": "secret"}
        with unittest.mock.patch.dict(os.environ, environment):
            return BUNDLE.main(["absent", "--registry", server.url, "--repository", "owner/packages", "--tag", "2026-10-22-abc"])

    def test_only_a_registry_not_found_proves_absence(self):
        unknown = lambda code: json.dumps({"errors": [{"code": code}]}).encode()
        self.assertEqual(self.check(404, unknown("MANIFEST_UNKNOWN")), 0)
        self.assertEqual(self.check(404, unknown("NAME_UNKNOWN")), 0)
        self.assertEqual(self.check(200, b"{}"), 1)
        self.assertEqual(self.check(403, unknown("DENIED")), 1)
        self.assertEqual(self.check(404, b"not json"), 1)
        self.assertEqual(self.check(500, b""), 1)


if __name__ == "__main__":
    unittest.main()
