"""The Omarchy key expiry check warns, then fails by name, and never passes without an answer."""

import contextlib
import hashlib
import importlib.util
import io
import json
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


def run_main(*arguments):
    """(exit code, stdout, stderr, step summary) of one run, never touching the real step summary."""
    with tempfile.TemporaryDirectory() as directory:
        summary = Path(directory) / "summary.md"
        out, err = io.StringIO(), io.StringIO()
        with unittest.mock.patch.dict(os.environ, {"GITHUB_STEP_SUMMARY": str(summary)}), \
                contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = CHECK.main(list(arguments))
        return code, out.getvalue(), err.getvalue(), summary.read_text() if summary.exists() else ""


@contextlib.contextmanager
def offline():
    """Fail any download, gpg or other subprocess, so a run that passes is provably offline."""
    refuse = unittest.mock.Mock(side_effect=AssertionError("the offline check fetched or ran a tool"))
    with unittest.mock.patch.object(CHECK.BUNDLE, "download", refuse), \
            unittest.mock.patch.object(CHECK.subprocess, "run", refuse):
        yield refuse


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

    def test_the_boundaries_follow_gpg(self):
        key = "D" * 40
        self.assertTrue(CHECK.verdict({key: (T0 + 14 * DAY + 1, 1)}, T0, 14)[1][0].startswith("Arch packager keys OK:"))
        self.assertTrue(CHECK.verdict({key: (T0 + 14 * DAY, 1)}, T0, 14)[1][0].startswith("::warning "))
        self.assertTrue(CHECK.verdict({key: (T0 + 1, 1)}, T0, 0)[1][0].startswith("Arch packager keys OK:"))
        # gpg still accepts a signature at the second its key expires, and refuses it one second later.
        self.assertEqual(CHECK.verdict({key: (T0, 1)}, T0, 14)[:2], (0, [
            f"::warning title=Arch packager key expires soon::Key {key} signing 1 pinned packages expires "
            "2026-01-01T00:00:00Z (0 days). Bump ARCH_SNAPSHOT before then; see #3040."]))
        self.assertEqual(CHECK.verdict({key: (T0 - 1, 1)}, T0, 14)[0], 1)

    def test_the_fail_window_turns_the_warning_into_a_named_error(self):
        found = {"E" * 40: (T0 + 20 * DAY, 3), "F" * 40: (T0 + 10 * DAY, 2)}
        code, lines, _ = CHECK.verdict(found, T0, 14, fail_days=30)
        self.assertEqual(code, 1)
        self.assertEqual(lines, [
            f"::error title=Arch packager key expires soon::Key {'F' * 40} signing 2 pinned packages expires "
            "2026-01-11T00:00:00Z (10 days). Bump ARCH_SNAPSHOT before then; see #3040.",
            f"::error title=Arch packager key expires soon::Key {'E' * 40} signing 3 pinned packages expires "
            "2026-01-21T00:00:00Z (20 days). Bump ARCH_SNAPSHOT before then; see #3040."])
        code, lines, _ = CHECK.verdict(found, T0, 14, fail_days=5)
        self.assertEqual((code, [line[:10] for line in lines]), (0, ["::warning "]))


class RecordTests(unittest.TestCase):
    """The committed record, read offline: no download, no gpg."""

    def setUp(self):
        self.lock, self.raw = CHECK.BUNDLE.load()
        self.record = CHECK.RECORD.read_bytes()

    def test_the_committed_record_is_bound_to_the_committed_lock(self):
        found = CHECK.recorded(self.lock, self.raw, CHECK.RECORD)
        self.assertEqual(CHECK.render(self.raw, found).encode(), self.record)
        self.assertEqual(sum(count for _, count in found.values()), sum(1 for e in self.lock["files"] if e["kind"] == "package"))
        earliest = min(expires for expires, _ in found.values() if expires is not None)
        with offline() as refused:
            code, out, err, summary = run_main("--now", iso(earliest - 30 * DAY))
        refused.assert_not_called()
        self.assertEqual((code, err), (0, ""))
        self.assertTrue(out.startswith("Arch packager keys OK:") and summary == "- " + out, out)

    def test_a_record_that_does_not_match_the_lock_is_refused(self):
        text = self.record.decode()
        first = json.loads(text)["keys"][0]
        line = json.dumps(first, sort_keys=True, separators=(",", ":"))
        cases = {
            "was derived from another lock": text.replace(hashlib.sha256(self.raw).hexdigest(), "0" * 64),
            "is not in canonical form": text.replace('"keys": [', '"keys":  ['),
            "does not cover every pinned package": text.replace(
                line, line.replace(f'"packages":{first["packages"]}', f'"packages":{first["packages"] + 1}')),
            "schema is not supported": text.replace(CHECK.RECORD_SCHEMA, "other"),
            "is not valid JSON": text[:-3],
            "has a malformed key": text.replace(line, line.replace('"fingerprint"', '"fpr"')),
        }
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "record.json"
            for message, changed in cases.items():
                with self.subTest(message), offline():
                    self.assertNotEqual(changed, text)
                    path.write_text(changed)
                    code, out, err, summary = run_main("--record", str(path), "--now", "2026-01-01T00:00:00Z")
                    self.assertEqual((code, out, summary), (2, "", ""))
                    self.assertIn(f"::error title=Arch packager key check failed::{path.name} {message}", err)
                    if message in ("was derived from another lock", "is not in canonical form"):
                        self.assertIn("run omarchy_key_expiry.py --refresh", err)
            with offline():
                code, _, err, _ = run_main("--record", str(Path(directory) / "missing.json"))
            self.assertEqual(code, 2)
            self.assertIn("::error title=Arch packager key check failed::FileNotFoundError: ", err)

    def test_an_unexpected_failure_exits_2_with_a_named_error(self):
        with unittest.mock.patch.object(CHECK, "verdict", side_effect=KeyError("boom")):
            code, out, err, summary = run_main()
        self.assertEqual((code, out, summary), (2, "", ""))
        self.assertEqual(err, "::error title=Arch packager key check failed::KeyError: 'boom'\n")


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
class DerivationTests(unittest.TestCase):
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
        self.record = self.work / "record.json"

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

    def derive(self, lock, cache, mode="--refresh"):
        return run_main("--lock", str(lock), "--cache", str(cache), "--record", str(self.record), mode, "--now", iso(T0))

    def check(self, lock, now, *extra):
        """An offline run against the derived record."""
        with offline() as refused:
            result = run_main("--lock", str(lock), "--record", str(self.record), "--now", iso(now), *extra)
        refused.assert_not_called()
        return result

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
        self.assertEqual(self.derive(lock, cache)[0], 0)
        code, out, err, summary = self.check(lock, T0 + DAY)
        line = (f"Arch packager keys OK: 4 signing keys cover 5 pinned packages; the earliest expiry is key "
                f"{self.keys['bounded']} on {iso(T0 + 50 * DAY)} (49 days).\n")
        self.assertEqual((code, out, err, summary), (0, line, "", "- " + line))

    def test_within_the_warning_window_the_exact_annotation_is_printed(self):
        lock, cache, _ = self.tree()
        self.derive(lock, cache)
        code, out, _, summary = self.check(lock, T0 + 40 * DAY)
        # The subkey itself expires on day 60; its primary key expires on day 50, and so does the subkey's use.
        message = (f"Key {self.keys['bounded']} signing 1 pinned packages expires {iso(T0 + 50 * DAY)} (10 days). "
                   "Bump ARCH_SNAPSHOT before then; see #3040.")
        self.assertEqual((code, out), (0, f"::warning title=Arch packager key expires soon::{message}\n"))
        self.assertEqual(summary, f"- {message}\n")
        code, out, *_ = self.check(lock, T0 + 40 * DAY, "--warn-days", "9")
        self.assertEqual(code, 0)
        self.assertTrue(out.startswith("Arch packager keys OK:"), out)
        code, out, *_ = self.check(lock, T0 + 40 * DAY, "--fail-days", "30")
        self.assertEqual((code, out), (1, f"::error title=Arch packager key expires soon::{message}\n"))

    def test_after_expiry_the_check_fails_by_name(self):
        lock, cache, _ = self.tree()
        self.derive(lock, cache)
        code, out, _, summary = self.check(lock, T0 + 55 * DAY)
        message = (f"Key {self.keys['bounded']} expired {iso(T0 + 50 * DAY)}; 1 pinned packages can no longer be verified. "
                   "Bump ARCH_SNAPSHOT per #3040.")
        self.assertEqual((code, out, summary), (1, f"::error title=Arch packager key expired::{message}\n", f"- {message}\n"))
        code, out, *_ = self.check(lock, T0 + 95 * DAY)
        self.assertEqual(code, 1)
        self.assertEqual([line.split("::")[1] for line in out.splitlines()],
                         ["error title=Arch packager key expired", "warning title=Arch packager key expires soon"])
        self.assertIn(f"Key {self.keys['packager']} signing 1 pinned packages expires {iso(T0 + 100 * DAY)} (5 days)", out)

    def test_a_key_that_signs_no_pinned_package_is_ignored(self):
        lock, cache, _ = self.tree()
        self.derive(lock, cache)
        self.assertNotIn(self.keys["bystander"], self.record.read_text())
        # The bystander key in the Arch keyring expired on day 10.
        code, out, *_ = self.check(lock, T0 + 20 * DAY)
        self.assertEqual(code, 0)
        self.assertNotIn(self.keys["bystander"], out)
        self.assertIn(f"key {self.keys['bounded']} on {iso(T0 + 50 * DAY)} (30 days)", out)

    def test_refresh_is_deterministic_and_verify_record_detects_drift(self):
        lock, cache, _ = self.tree()
        self.derive(lock, cache)
        first = self.record.read_bytes()
        self.derive(lock, cache)
        self.assertEqual(self.record.read_bytes(), first)
        self.assertEqual(self.derive(lock, cache, "--verify-record")[0], 0)
        record = json.loads(first)
        self.assertEqual([key["fingerprint"] for key in record["keys"]], sorted(key["fingerprint"] for key in record["keys"]))
        self.assertEqual(record["lock_sha256"], hashlib.sha256(lock.read_bytes()).hexdigest())
        # A hand edit can keep the record canonical and complete, so only re-deriving it finds the drift.
        stale = first.replace(iso(T0 + 50 * DAY).encode(), iso(T0 + 500 * DAY).encode())
        self.record.write_bytes(stale)
        self.assertEqual(self.check(lock, T0 + 40 * DAY)[0], 0)
        code, out, err, _ = self.derive(lock, cache, "--verify-record")
        self.assertEqual((code, out), (2, ""))
        self.assertIn("record.json differs from the keys that sign the locked packages; run omarchy_key_expiry.py --refresh", err)
        self.assertEqual(self.record.read_bytes(), stale)

    def test_files_are_fetched_by_their_locked_urls(self):
        lock, cache, files = self.tree()
        by_name = {Path(path).name: data for path, data in files.items()}
        shutil.rmtree(cache)
        patch, requested = self.serve(lambda url: by_name[url.rsplit("/", 1)[1]])
        with patch:
            code, out, *_ = self.derive(lock, cache)
        self.assertEqual(code, 0, out)
        wanted = {name for name in by_name if name.endswith(".sig") or "-keyring-" in name}
        self.assertEqual(sorted(url.rsplit("/", 1)[1] for url in requested), sorted(wanted))
        self.assertTrue(all(url.startswith("https://example.invalid/") for url in requested))

    def test_every_download_or_keyring_problem_fails_closed(self):
        lock, cache, _ = self.tree()
        signature = cache / "core/os/x86_64/app-1-1-x86_64.pkg.tar.zst.sig"
        original = signature.read_bytes()
        # Invert the last byte. Overwriting it with a constant is a no-op whenever
        # the freshly generated signature already ends in that byte; the cached
        # copy then still matches the lock and the check succeeds, failing this
        # test in roughly one run in 256.
        signature.write_bytes(original[:-1] + bytes([original[-1] ^ 0xFF]))
        self.assertNotEqual(signature.read_bytes(), original)
        for mode in ("--refresh", "--verify-record"):
            patch, _ = self.serve(lambda url: b"other bytes")
            with self.subTest(mode), patch:
                code, out, err, _ = self.derive(lock, cache, mode)
            self.assertEqual((code, out), (2, ""))
            self.assertIn("::error title=Arch packager key check failed::", err)
            self.assertIn("content does not match the lock", err)
        patch, _ = self.serve(lambda url: 6)  # curl: could not resolve host
        with patch:
            code, _, err, _ = self.derive(lock, cache)
        self.assertEqual(code, 2)
        self.assertIn("curl exit 6", err)
        cases = {
            "lacks its keyring files": {"signers": None, "arch_ring": {"usr/share/pacman/keyrings/archlinux-revoked": b""}},
            "is not in its pinned keyring": {"signers": {"app": "stranger"}, "arch_ring": None},
        }
        for message, arguments in cases.items():
            with self.subTest(message):
                lock, cache, _ = self.tree(**arguments)
                code, out, err, summary = self.derive(lock, cache)
                self.assertEqual((code, out, summary), (2, "", ""))
                self.assertIn(message, err)
        self.assertFalse(self.record.exists(), "a failed refresh must not write a record")


if __name__ == "__main__":
    unittest.main()
