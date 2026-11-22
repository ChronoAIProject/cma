"""The Omarchy key expiry check warns, then fails by name, and never passes without an answer."""

import contextlib
import hashlib
import importlib.util
import io
import os
import shutil
import subprocess
import tarfile
import tempfile
import time
import unittest
import unittest.mock
from pathlib import Path

SCRIPT = Path(__file__).with_name("omarchy_key_expiry.py")
SPEC = importlib.util.spec_from_file_location("omarchy_key_expiry", SCRIPT)
CHECK = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CHECK)
TOOLS = all(shutil.which(tool) for tool in ("gpg", "gpgconf", "zstd"))
DAY = 86400
T0 = 1767225600  # 2026-01-01T00:00:00Z: every synthetic key is created then.


def iso(epoch):
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(epoch))


def subpacket(kind, value):
    size = len(value) + 1
    if size < 192:
        return bytes([size, kind]) + value
    return bytes([((size - 192) >> 8) + 192, (size - 192) & 0xFF, kind]) + value


def signature(hashed=b"", unhashed=b"", version=4, new_format=False):
    """A detached signature packet; the check reads only its header and subpackets."""
    body = (bytes([version, 0x00, 22, 10]) + len(hashed).to_bytes(2, "big") + hashed
            + len(unhashed).to_bytes(2, "big") + unhashed + b"\x12\x34" + b"\x00\x08\xff")
    if new_format:
        assert 192 <= len(body) < 8384
        size = len(body) - 192
        return bytes([0xC2, (size >> 8) + 192, size & 0xFF]) + body
    return bytes([0x89]) + len(body).to_bytes(2, "big") + body


FPR = bytes(range(1, 21))
FPR_HEX = FPR.hex().upper()


class IssuerTests(unittest.TestCase):
    def test_the_issuer_fingerprint_names_the_signing_key(self):
        self.assertEqual(CHECK.issuer(signature(subpacket(33, b"\x04" + FPR))), FPR_HEX)
        both = signature(subpacket(33, b"\x04" + FPR), subpacket(16, FPR[-8:]))
        self.assertEqual(CHECK.issuer(both), FPR_HEX)

    def test_a_long_key_id_is_used_when_no_fingerprint_is_given(self):
        padding = subpacket(20, b"n" * 200)  # a notation long enough for two-octet subpacket and packet lengths
        self.assertEqual(CHECK.issuer(signature(padding, subpacket(16, FPR[-8:]), new_format=True)), FPR[-8:].hex().upper())

    def test_anything_but_one_v4_signature_naming_one_issuer_is_refused(self):
        good = signature(subpacket(33, b"\x04" + FPR))
        cases = {
            "no issuer": signature(subpacket(2, b"\x00\x00\x00\x01")),
            "two issuers": signature(subpacket(33, b"\x04" + FPR), subpacket(16, b"\x99" * 8)),
            "version 3": signature(subpacket(33, b"\x04" + FPR), version=3),
            "two signatures": good + good,
            "a public key": bytes([0x99]) + good[1:],
            "truncated": good[:-4],
            "armored": b"-----BEGIN PGP SIGNATURE-----\n",
            "empty": b"",
        }
        for name, data in cases.items():
            with self.subTest(name), self.assertRaises(CHECK.BundleError):
                CHECK.issuer(data)


class VerdictTests(unittest.TestCase):
    def test_ok_names_the_earliest_expiry(self):
        found = {"A" * 40: (T0 + 30 * DAY, 3), "B" * 40: (T0 + 20 * DAY, 2), "C" * 40: (None, 4)}
        code, lines, summary = CHECK.verdict(found, T0, 14)
        self.assertEqual(code, 0)
        self.assertEqual(lines, ["Arch packager keys OK: 3 signing keys cover 9 pinned packages; the earliest expiry is key "
                                 + "B" * 40 + " on 2026-01-21T00:00:00Z (20 days)."])
        self.assertEqual(summary, lines)
        self.assertIn("none of them expires", CHECK.verdict({"C" * 40: (None, 4)}, T0, 14)[1][0])

    def test_the_boundaries_of_warning_and_expiry(self):
        key = "D" * 40
        self.assertEqual(CHECK.verdict({key: (T0 + 14 * DAY + 1, 1)}, T0, 14)[1][0][:22], "Arch packager keys OK:")
        self.assertTrue(CHECK.verdict({key: (T0 + 14 * DAY, 1)}, T0, 14)[1][0].startswith("::warning "))
        self.assertTrue(CHECK.verdict({key: (T0 + 1, 1)}, T0, 0)[1][0].startswith("Arch packager keys OK:"))
        self.assertEqual(CHECK.verdict({key: (T0, 1)}, T0, 14)[0], 1)


def fake_package(files):
    """A zstd-compressed tar with the given members, as pacman packages are."""
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w") as t:
        for name, data in files.items():
            info = tarfile.TarInfo(name)
            info.size = len(data)
            t.addfile(info, io.BytesIO(data))
    return subprocess.run(["zstd", "-q", "-c"], input=buffer.getvalue(), capture_output=True, check=True).stdout


# Only a local run may skip these. Under CI a missing tool fails them, so a
# runner image that drops gpg or zstd cannot pass silently.
@unittest.skipUnless(TOOLS or os.environ.get("CI"), "gpg and zstd are required")
class CheckTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        assert TOOLS, "CI is set, so gpg and zstd must be installed"
        cls.directory = tempfile.TemporaryDirectory()
        cls.home = Path(cls.directory.name) / "gnupg"
        cls.home.mkdir(mode=0o700)
        days = lambda n: f"seconds={n * DAY}"
        # The packager signs with a subkey, as Arch packagers do; the bounded
        # key's primary expires ten days before its signing subkey.
        cls.keys = {"packager": cls.subkey("packager", "never", days(100)),
                    "bounded": cls.subkey("bounded", days(50), days(60)),
                    "maintainer": cls.primary("maintainer", days(3 * 365)),
                    "bystander": cls.primary("bystander", days(10)),
                    "omarchy": cls.primary("omarchy", "never"),
                    "stranger": cls.primary("stranger", "never")}

    @classmethod
    def tearDownClass(cls):
        subprocess.run(["gpgconf", "--homedir", str(cls.home), "--kill", "all"], capture_output=True, check=False)
        cls.directory.cleanup()

    @classmethod
    def gpg(cls, *args, data=None):
        return subprocess.run(["gpg", "--homedir", str(cls.home), "--batch", "--quiet", "--pinentry-mode", "loopback",
                               "--passphrase", "", "--faked-system-time", f"{T0}!", *args],
                              input=data, capture_output=True, check=True).stdout

    @classmethod
    def fingerprints(cls, name):
        listing = cls.gpg("--with-colons", "--with-subkey-fingerprint", "--list-keys", f"{name}@example.invalid").decode()
        return [line.split(":")[9] for line in listing.splitlines() if line.startswith("fpr")]

    @classmethod
    def primary(cls, name, expire):
        cls.gpg("--quick-gen-key", f"{name} <{name}@example.invalid>", "ed25519", "sign", expire)
        return cls.fingerprints(name)[0]

    @classmethod
    def subkey(cls, name, primary_expire, subkey_expire):
        cls.gpg("--quick-gen-key", f"{name} <{name}@example.invalid>", "ed25519", "cert", primary_expire)
        cls.gpg("--quick-add-key", cls.fingerprints(name)[0], "ed25519", "sign", subkey_expire)
        return cls.fingerprints(name)[1]

    def setUp(self):
        self.work = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.work)

    def sign(self, name, data):
        return self.gpg("--local-user", self.keys[name] + "!", "--detach-sign", "--output", "-", data=data)

    def keyring(self, *names, armor=False):
        return self.gpg(*(["--armor"] if armor else []), "--export", *(self.keys[n] for n in names))

    def tree(self, signers=None, arch_ring=None):
        """Write a canonical lock and a cache holding every file it pins; return (lock path, cache, lock files)."""
        signers = signers or {"app": "packager", "lib": "bounded", "tool": "maintainer"}
        arch_ring = arch_ring or {"usr/share/pacman/keyrings/archlinux.gpg":
                                  self.keyring("packager", "bounded", "maintainer", "bystander", armor=True),
                                  "usr/share/pacman/keyrings/archlinux-revoked": b""}
        rings = {"core/os/x86_64/archlinux-keyring-1-1-any.pkg.tar.zst": (fake_package(arch_ring), "maintainer"),
                 "omarchy/x86_64/omarchy-keyring-1-1-any.pkg.tar.zst": (fake_package({
                     "usr/share/pacman/keyrings/omarchy.gpg": self.keyring("omarchy"),
                     "usr/share/pacman/keyrings/omarchy-revoked": b""}), "omarchy")}
        packages = dict(rings)
        for name, signer in signers.items():
            packages[f"core/os/x86_64/{name}-1-1-x86_64.pkg.tar.zst"] = (fake_package({f"usr/bin/{name}": b"binary"}), signer)
        files = {"core/os/x86_64/core.db": b"database"}
        entries = [{"path": "core/os/x86_64/core.db", "kind": "database"}]
        for path, (data, signer) in packages.items():
            files[path], files[path + ".sig"] = data, self.sign(signer, data)
            entry = {"path": path, "kind": "package", "signature": path + ".sig",
                     "keyring": "omarchy" if path.startswith("omarchy/") else "arch"}
            if path.startswith("core/"):
                entry["database"] = "core/os/x86_64/core.db"
            entries += [entry, {"path": path + ".sig", "kind": "signature"}]
        for entry in entries:
            data = files[entry["path"]]
            entry.update(size=len(data), sha256=hashlib.sha256(data).hexdigest(),
                         urls=["https://example.invalid/" + Path(entry["path"]).name])
        lock = {"schema": CHECK.BUNDLE.SCHEMA, "snapshot": "2026/10/22", "files": sorted(entries, key=lambda e: e["path"]),
                "keyrings": {"arch": "core/os/x86_64/archlinux-keyring-1-1-any.pkg.tar.zst",
                             "omarchy": "omarchy/x86_64/omarchy-keyring-1-1-any.pkg.tar.zst"}}
        lock_path = self.work / "lock.json"
        lock_path.write_text(CHECK.BUNDLE.canonical(lock))
        cache = self.work / "cache"
        for path, data in files.items():
            (cache / path).parent.mkdir(parents=True, exist_ok=True)
            (cache / path).write_bytes(data)
        return lock_path, cache, files

    def run_check(self, lock, cache, now, *extra):
        summary = self.work / "summary.md"
        summary.unlink(missing_ok=True)
        out, err = io.StringIO(), io.StringIO()
        with unittest.mock.patch.dict(os.environ, {"GITHUB_STEP_SUMMARY": str(summary)}), \
                contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = CHECK.main(["--lock", str(lock), "--cache", str(cache), "--now", iso(now), *extra])
        return code, out.getvalue(), err.getvalue(), summary.read_text() if summary.exists() else ""

    def serve(self, answer):
        """Answer the builder's curl for each locked URL with answer(url): bytes, or a curl exit code."""
        original, requested = CHECK.BUNDLE.fetch, []

        def curl(arguments, **_):
            url = arguments[-1]
            requested.append(url)
            reply = answer(url)
            if isinstance(reply, int):
                return subprocess.CompletedProcess(arguments, reply, "", "")
            Path(arguments[arguments.index("--output") + 1]).write_bytes(reply)
            return subprocess.CompletedProcess(arguments, 0, "", "")

        def fetch(urls, target, size, digest, attempts, delay, hosts=None, stop=None):
            return original(urls, target, size, digest, 2, 0, runner=curl, sleep=lambda _: None, hosts=hosts, stop=stop)

        return unittest.mock.patch.object(CHECK.BUNDLE, "fetch", fetch), requested

    def test_far_from_expiry_the_check_names_the_earliest_signing_key_expiry(self):
        lock, cache, _ = self.tree()
        code, out, err, summary = self.run_check(lock, cache, T0 + DAY)
        line = (f"Arch packager keys OK: 4 signing keys cover 5 pinned packages; the earliest expiry is key "
                f"{self.keys['bounded']} on {iso(T0 + 50 * DAY)} (49 days).\n")
        self.assertEqual((code, out, err, summary), (0, line, "", "- " + line))

    def test_within_the_warning_window_the_exact_annotation_is_printed(self):
        lock, cache, _ = self.tree()
        code, out, _, summary = self.run_check(lock, cache, T0 + 40 * DAY)
        # The subkey itself expires on day 60; its primary key expires on day 50, and so does the subkey's use.
        message = (f"Key {self.keys['bounded']} signing 1 pinned packages expires {iso(T0 + 50 * DAY)} (10 days). "
                   "Bump ARCH_SNAPSHOT before then; see #3040.")
        self.assertEqual((code, out), (0, f"::warning title=Arch packager key expires soon::{message}\n"))
        self.assertEqual(summary, f"- {message}\n")
        code, out, *_ = self.run_check(lock, cache, T0 + 40 * DAY, "--warn-days", "9")
        self.assertEqual(code, 0)
        self.assertTrue(out.startswith("Arch packager keys OK:"), out)

    def test_after_expiry_the_check_fails_by_name(self):
        lock, cache, _ = self.tree()
        code, out, _, summary = self.run_check(lock, cache, T0 + 55 * DAY)
        message = (f"Key {self.keys['bounded']} expired {iso(T0 + 50 * DAY)}; 1 pinned packages can no longer be verified. "
                   "Bump ARCH_SNAPSHOT per #3040.")
        self.assertEqual((code, out, summary), (1, f"::error title=Arch packager key expired::{message}\n", f"- {message}\n"))
        code, out, *_ = self.run_check(lock, cache, T0 + 95 * DAY)
        self.assertEqual(code, 1)
        self.assertEqual([line.split("::")[1] for line in out.splitlines()],
                         ["error title=Arch packager key expired", "warning title=Arch packager key expires soon"])
        self.assertIn(f"Key {self.keys['packager']} signing 1 pinned packages expires {iso(T0 + 100 * DAY)} (5 days)", out)

    def test_a_key_that_signs_no_pinned_package_is_ignored(self):
        lock, cache, _ = self.tree()
        # The bystander key in the Arch keyring expired on day 10.
        code, out, *_ = self.run_check(lock, cache, T0 + 20 * DAY)
        self.assertEqual(code, 0)
        self.assertNotIn(self.keys["bystander"], out)
        self.assertIn(f"key {self.keys['bounded']} on {iso(T0 + 50 * DAY)} (30 days)", out)

    def test_files_are_fetched_by_their_locked_urls(self):
        lock, cache, files = self.tree()
        by_name = {Path(path).name: data for path, data in files.items()}
        shutil.rmtree(cache)
        patch, requested = self.serve(lambda url: by_name[url.rsplit("/", 1)[1]])
        with patch:
            code, out, *_ = self.run_check(lock, cache, T0 + DAY)
        self.assertEqual(code, 0, out)
        wanted = {name for name in by_name if name.endswith(".sig") or "-keyring-" in name}
        self.assertEqual(sorted(url.rsplit("/", 1)[1] for url in requested), sorted(wanted))
        self.assertTrue(all(url.startswith("https://example.invalid/") for url in requested))

    def test_every_download_or_keyring_problem_fails_closed(self):
        lock, cache, _ = self.tree()
        signature = cache / "core/os/x86_64/app-1-1-x86_64.pkg.tar.zst.sig"
        signature.write_bytes(signature.read_bytes()[:-1] + b"\x00")
        patch, _ = self.serve(lambda url: b"other bytes")
        with patch:
            code, out, err, _ = self.run_check(lock, cache, T0 + DAY)
        self.assertEqual((code, out), (2, ""))
        self.assertIn("::error title=Arch packager key check failed::", err)
        self.assertIn("content does not match the lock", err)
        patch, _ = self.serve(lambda url: 6)  # curl: could not resolve host
        with patch:
            code, _, err, _ = self.run_check(lock, cache, T0 + DAY)
        self.assertEqual(code, 2)
        self.assertIn("curl exit 6", err)
        cases = {
            "lacks its keyring files": {"signers": None, "arch_ring": {"usr/share/pacman/keyrings/archlinux-revoked": b""}},
            "is not in its pinned keyring": {"signers": {"app": "stranger"}, "arch_ring": None},
        }
        for message, arguments in cases.items():
            with self.subTest(message):
                lock, cache, _ = self.tree(**arguments)
                code, out, err, summary = self.run_check(lock, cache, T0 + DAY)
                self.assertEqual((code, out, summary), (2, "", ""))
                self.assertIn(message, err)


if __name__ == "__main__":
    unittest.main()
