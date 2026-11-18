"""Prove CA-only certificate bytes in each saved layer state, without extraction."""

import base64
import binascii
import hashlib
import posixpath
import re
import subprocess
import tarfile

from .common import ScanError, archive_path

ROOTS = ("etc/ssl/certs", "usr/share/ca-certificates", "usr/local/share/ca-certificates")
ALIASES = frozenset({"etc/ssl/cert.pem", "etc/ssl1.1/cert.pem", "usr/lib/ssl/cert.pem"})
PEM = re.compile(
    rb"-----BEGIN CERTIFICATE-----[ \t\r\n]*(?P<body>[A-Za-z0-9+/= \t\r\n]+)-----END CERTIFICATE-----"
)
MAX_FILE = 1024 * 1024
MAX_STATE = 16 * 1024 * 1024


def allowed_path(path):
    return path in ALIASES or any(path == root or path.startswith(root + "/") for root in ROOTS)


def certificate_proof(data):
    blocks = list(PEM.finditer(data))
    if not blocks or PEM.sub(b"", data).strip():
        return None
    fingerprints = []
    for block in blocks:
        compact = re.sub(rb"[ \t\r\n]", b"", block["body"])
        try:
            decoded = base64.b64decode(compact, validate=True)
        except (ValueError, binascii.Error):
            return None
        # Strict round-trip rejects excess padding and nonzero discarded bits.
        if not decoded or base64.b64encode(decoded) != compact:
            return None
        try:
            parsed = subprocess.run(
                ["openssl", "x509", "-inform", "DER", "-outform", "DER"],
                input=decoded,
                capture_output=True,
                timeout=10,
                check=False,
            )
            constraints = subprocess.run(
                ["openssl", "x509", "-inform", "DER", "-noout", "-ext", "basicConstraints"],
                input=decoded,
                capture_output=True,
                timeout=10,
                check=False,
            )
        except (OSError, subprocess.TimeoutExpired) as error:
            raise ScanError("certificate-reader-unavailable") from error
        # OpenSSL accepts a valid certificate prefix. Equality is required to
        # prove that every decoded byte belongs to that one canonical object.
        if parsed.returncode or parsed.stdout != decoded:
            return None
        # This command requests one parsed ASN.1 extension, never a subject or
        # arbitrary extension rendering that could spoof a text match.
        if constraints.returncode:
            raise ScanError("certificate-constraints-reader-failed")
        if not re.fullmatch(
            rb"X509v3 Basic Constraints:(?: critical)?\s+CA:TRUE(?:, pathlen:[0-9]+)?\s*", constraints.stdout
        ):
            return None
        fingerprints.append(hashlib.sha256(parsed.stdout).hexdigest())
    return {"contents_sha256": hashlib.sha256(data).hexdigest(), "certificates": fingerprints}


class CertificateState:
    def __init__(self):
        self.files = {}
        self.links = {}
        self.cache = {}
        self.ambiguous = set()

    def remove(self, path):
        for mapping in (self.files, self.links):
            for name in list(mapping):
                if name == path or name.startswith(path + "/"):
                    del mapping[name]

    def resolve(self, path):
        seen = set()
        for _ in range(40):
            if path in seen or not allowed_path(path):
                return None
            seen.add(path)
            parts = path.split("/")
            for length in range(1, len(parts) + 1):
                prefix = "/".join(parts[:length])
                if prefix in self.ambiguous:
                    return None
                if prefix not in self.links:
                    continue
                target = self.links[prefix]
                target = (
                    target.lstrip("/")
                    if target.startswith("/")
                    else posixpath.join(posixpath.dirname(prefix), target)
                )
                path = posixpath.normpath(posixpath.join(target, *parts[length:]))
                break
            else:
                return self.files.get(path)
        return None

    def layer(self, path, image, layer, findings):
        candidates = []
        seen = set()
        self.ambiguous = set()
        with tarfile.open(path, "r:") as archive:
            for index, entry in enumerate(archive):
                name = archive_path(entry.name)
                base = posixpath.basename(name)
                if base == ".wh..wh..opq":
                    self.remove(posixpath.dirname(name))
                    continue
                if base.startswith(".wh."):
                    self.remove(posixpath.join(posixpath.dirname(name), base[4:]))
                    continue
                if not allowed_path(name):
                    continue
                if not entry.isdir():
                    if name in seen:
                        self.ambiguous.add(name)
                    seen.add(name)
                    self.remove(name)
                if entry.issym():
                    self.links[name] = entry.linkname
                elif entry.islnk():
                    # Hardlinks bind an inode at entry time, not a future path.
                    # Keep them unresolved rather than infer symlink semantics.
                    self.ambiguous.add(name)
                elif entry.isfile() and entry.size <= MAX_FILE:
                    with archive.extractfile(entry) as stream:
                        self.files[name] = stream.read(MAX_FILE + 1)
                if sum(map(len, self.files.values())) > MAX_STATE or len(self.files) + len(self.links) > 8192:
                    raise ScanError("certificate-state-limit")
                if name.lower().endswith((".pem", ".key", ".p12", ".pfx")):
                    candidates.append((index, name))
        # Classify against the complete current layer, never a later layer.
        for index, name in candidates:
            data = self.resolve(name)
            if data is None:
                continue
            digest = hashlib.sha256(data).hexdigest()
            if digest not in self.cache:
                self.cache[digest] = certificate_proof(data)
            if self.cache[digest] is not None:
                findings.public_ca[(image, layer, index, name)] = self.cache[digest]
