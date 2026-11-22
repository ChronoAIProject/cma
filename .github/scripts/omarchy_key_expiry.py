"""Warn before, and fail after, a key that signs a pinned Omarchy package expires.

The Omarchy image installs Arch and Omarchy packages only from the bundle that
omarchy-package-bundle.py builds from the committed lock, and pacman verifies
every package signature against the keyring packages that lock pins. The lock
and its keyrings are fixed, so the one thing that changes with time is whether
the keys that made those signatures have expired. Once one has, pacman refuses
the packages it signed and the bundle can no longer be rebuilt; the remedy is a
deliberate ARCH_SNAPSHOT bump (#3040).

The keyring packages and every package signature are fetched by their locked
URLs and checked by size and sha256 with the builder's own download. Each
signature names its signing key or subkey; gpg reads that key's expiry from
the pinned keyring, and a subkey expires no later than its primary key, as gpg
treats it. Keys that sign no pinned package are ignored.

Exit 0 is a pass, with a warning annotation for each key that expires within
--warn-days; exit 1 names each key that has expired. Any download, keyring or
parse failure exits 2: the check never passes without an answer.
"""

import argparse
import datetime
import importlib.util
import os
import subprocess
import sys
import tarfile
import tempfile
import time
from pathlib import Path

SPEC = importlib.util.spec_from_file_location("omarchy_package_bundle", Path(__file__).with_name("omarchy-package-bundle.py"))
BUNDLE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BUNDLE)
require, BundleError = BUNDLE.require, BUNDLE.BundleError

DAY = 86400
PARALLEL = 8
ISSUE = "#3040"
SIGNATURE, ISSUER_KEY_ID, ISSUER_FINGERPRINT = 2, 16, 33


def length(data, i, subpacket):
    """An OpenPGP length at data[i]: (length, next index). 224-254 open a partial packet body, but not a subpacket."""
    require(i < len(data), "OpenPGP data is truncated")
    first = data[i]
    if first < 192:
        return first, i + 1
    if first < 224 or (first < 255 and subpacket):
        require(i + 1 < len(data), "OpenPGP data is truncated")
        return ((first - 192) << 8) + data[i + 1] + 192, i + 2
    require(first == 255, "OpenPGP partial body lengths are not a signature")
    require(i + 5 <= len(data), "OpenPGP data is truncated")
    return int.from_bytes(data[i + 1:i + 5], "big"), i + 5


def packets(data):
    i = 0
    while i < len(data):
        head = data[i]
        require(head & 0x80, "not a binary OpenPGP packet")
        if head & 0x40:
            tag, (size, i) = head & 0x3F, length(data, i + 1, False)
        else:
            width = {0: 1, 1: 2, 2: 4}.get(head & 3)
            require(width is not None, "OpenPGP packet has an indeterminate length")
            tag, size, i = (head >> 2) & 0x0F, int.from_bytes(data[i + 1:i + 1 + width], "big"), i + 1 + width
        require(i + size <= len(data), "OpenPGP packet is truncated")
        yield tag, data[i:i + size]
        i += size


def subpackets(area):
    i = 0
    while i < len(area):
        size, i = length(area, i, True)
        require(size > 0 and i + size <= len(area), "OpenPGP subpacket is truncated")
        yield area[i] & 0x7F, area[i + 1:i + size]
        i += size


def issuer(data):
    """The fingerprint, or failing that the long key ID, of the key that made one detached v4 signature."""
    found = list(packets(data))
    require(len(found) == 1 and found[0][0] == SIGNATURE, "is not exactly one OpenPGP signature packet")
    body = found[0][1]
    require(len(body) >= 6 and body[0] == 4, "is not a version 4 OpenPGP signature")
    hashed = int.from_bytes(body[4:6], "big")
    require(len(body) >= 8 + hashed, "OpenPGP signature is truncated")
    unhashed = int.from_bytes(body[6 + hashed:8 + hashed], "big")
    require(len(body) >= 8 + hashed + unhashed, "OpenPGP signature is truncated")
    fingerprints, key_ids = set(), set()
    for area in (body[6:6 + hashed], body[8 + hashed:8 + hashed + unhashed]):
        for kind, value in subpackets(area):
            if kind == ISSUER_FINGERPRINT and len(value) == 21 and value[0] == 4:
                fingerprints.add(value[1:].hex().upper())
            elif kind == ISSUER_KEY_ID and len(value) == 8:
                key_ids.add(value.hex().upper())
    require(len(fingerprints) <= 1 and len(key_ids) <= 1, "OpenPGP signature names more than one issuer")
    if fingerprints:
        fingerprint = fingerprints.pop()
        require(all(fingerprint.endswith(k) for k in key_ids), "OpenPGP signature names two different issuers")
        return fingerprint
    require(key_ids, "OpenPGP signature names no issuer")
    return key_ids.pop()


def expiries(keyring, home):
    """Each key and subkey fingerprint of a binary keyring, with the time gpg treats it as expired (None: never)."""
    home.mkdir(mode=0o700)
    result = subprocess.run(["gpg", "--homedir", str(home), "--batch", "--no-default-keyring", "--keyring", str(keyring),
                             "--trust-model", "always", "--with-colons", "--fixed-list-mode", "--with-subkey-fingerprint",
                             "--list-keys"], capture_output=True, text=True, check=False)
    require(result.returncode == 0, f"gpg cannot list {keyring.name}: {result.stderr.strip()}")
    keys, primary, expires, listed = {}, None, None, False
    for fields in (line.split(":") for line in result.stdout.splitlines()):
        if fields[0] in ("pub", "sub"):
            # A key's own fpr record follows its pub or sub record.
            expires, listed = (int(fields[6]) if fields[6] else None), True
            if fields[0] == "pub":
                primary = expires
            elif primary is not None:
                expires = primary if expires is None else min(expires, primary)
        elif fields[0] == "fpr" and listed:
            keys[fields[9]], listed = expires, False
    require(keys, f"{keyring.name} holds no keys")
    return keys


def signing_key(keys, named, path):
    matches = [f for f in keys if f == named or (len(named) == 16 and f.endswith(named))]
    require(len(matches) == 1, f"{path}: signing key {named} is not in its pinned keyring")
    return matches[0]


def signers(lock, root, scratch):
    """Map every key that signs a pinned package to (expiry, number of pinned packages it signs)."""
    keys = {}
    for name, (ring, revoked) in BUNDLE.KEYRINGS.items():
        (scratch / name).mkdir()
        files = BUNDLE.extract_keyring(root / lock["keyrings"][name], (ring, revoked), scratch / name)
        keys[name] = expiries(files[ring], scratch / name / "gnupg")
    found = {}
    for e in lock["files"]:
        if e["kind"] != "package":
            continue
        try:
            named = issuer((root / e["signature"]).read_bytes())
        except BundleError as error:
            raise BundleError(f"{e['signature']}: {error}") from error
        fingerprint = signing_key(keys[e["keyring"]], named, e["path"])
        expires, count = found.get(fingerprint, (keys[e["keyring"]][fingerprint], 0))
        found[fingerprint] = (expires, count + 1)
    return found


def stamp(epoch):
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(epoch))


def verdict(found, now, warn_days):
    """(exit code, annotation lines, summary lines) for the signing keys at now."""
    errors, warnings = [], []
    for fingerprint, (expires, count) in sorted(found.items(), key=lambda item: (item[1][0] or 0, item[0])):
        if expires is None:
            continue
        if expires <= now:
            errors.append(f"Key {fingerprint} expired {stamp(expires)}; {count} pinned packages can no longer be verified. "
                          f"Bump ARCH_SNAPSHOT per {ISSUE}.")
        elif expires - now <= warn_days * DAY:
            warnings.append(f"Key {fingerprint} signing {count} pinned packages expires {stamp(expires)} "
                            f"({int((expires - now) // DAY)} days). Bump ARCH_SNAPSHOT before then; see {ISSUE}.")
    lines = [f"::error title=Arch packager key expired::{m}" for m in errors]
    lines += [f"::warning title=Arch packager key expires soon::{m}" for m in warnings]
    if errors or warnings:
        return (1 if errors else 0), lines, errors + warnings
    packages = sum(count for _, count in found.values())
    dated = [(expires, fingerprint) for fingerprint, (expires, _) in found.items() if expires is not None]
    if dated:
        expires, fingerprint = min(dated)
        earliest = f"the earliest expiry is key {fingerprint} on {stamp(expires)} ({int((expires - now) // DAY)} days)"
    else:
        earliest = "none of them expires"
    line = f"Arch packager keys OK: {len(found)} signing keys cover {packages} pinned packages; {earliest}."
    return 0, [line], [line]


def instant(text):
    try:
        moment = datetime.datetime.fromisoformat(text)
    except ValueError as error:
        raise argparse.ArgumentTypeError("--now must be an ISO 8601 timestamp") from error
    if moment.tzinfo is None:
        raise argparse.ArgumentTypeError("--now must name its time zone, such as Z")
    return moment.timestamp()


def days(text):
    value = int(text)
    if value < 0:
        raise argparse.ArgumentTypeError("--warn-days must not be negative")
    return value


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--warn-days", type=days, default=14, help="warn when a signing key expires within N days")
    parser.add_argument("--now", type=instant, help="ISO 8601 time to evaluate at (default: the current time)")
    parser.add_argument("--lock", default=BUNDLE.LOCK)
    parser.add_argument("--cache", help="directory of lock files by path: verified files are reused, downloads are kept")
    args = parser.parse_args(argv)
    now = time.time() if args.now is None else args.now
    try:
        lock, _ = BUNDLE.load(args.lock)
        by_path = {e["path"]: e for e in lock["files"]}
        needed = [by_path[p] for p in sorted(set(lock["keyrings"].values()))]
        needed += [by_path[e["signature"]] for e in lock["files"] if e["kind"] == "package"]
        with tempfile.TemporaryDirectory() as scratch:
            scratch = Path(scratch)
            root = Path(args.cache) if args.cache else scratch / "files"
            BUNDLE.download(needed, root, parallel=PARALLEL)
            found = signers(lock, root, scratch)
    except (BundleError, subprocess.CalledProcessError, OSError, ValueError, tarfile.TarError) as error:
        print(f"::error title=Arch packager key check failed::{error}", file=sys.stderr)
        return 2
    code, lines, summary = verdict(found, now, args.warn_days)
    print("\n".join(lines))
    if os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(os.environ["GITHUB_STEP_SUMMARY"], "a") as f:
            f.write("".join(f"- {line}\n" for line in summary))
    return code


if __name__ == "__main__":
    sys.exit(main())
