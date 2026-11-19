"""Prove certificate-only material against one image's archive namespace."""

import base64
import binascii
import hashlib
import re
from pathlib import PurePosixPath

from .key_material import decode, frame, has_binary_key

ROOTS = ("etc/ssl/certs", "usr/share/ca-certificates", "usr/local/share/ca-certificates",
         "etc/ca-certificates", "etc/pki")
BUNDLES = ("etc/ssl/cert.pem", "usr/lib/ssl/cert.pem")
ANCESTORS = {str(p) for name in (*ROOTS, *BUNDLES) for p in PurePosixPath(name).parents}
PEM = re.compile(rb"-----BEGIN CERTIFICATE-----[ \t]*\r?\n([A-Za-z0-9+/=\r\n]+)-----END CERTIFICATE-----")
SPACE = b" \r\n\t"
MAX_CERT_FILE = 4 * 1024 * 1024


def trust_path(path):
    return path in BUNDLES or any(path.startswith(root + "/") for root in ROOTS)


def certificate(data):
    outer = frame(data, 0, len(data))
    if not outer or outer[0] != 48 or outer[2] != len(data):
        return False
    if has_binary_key(data) or b"PRIVATE KEY-----" in data:
        return False
    parsed = decode(["x509", "-inform", "DER", "-outform", "DER"], data)
    return parsed.returncode == 0 and parsed.stdout == data


def certificate_digest(path):
    size = path.stat().st_size
    if not 0 < size <= MAX_CERT_FILE:
        return None
    data = path.read_bytes()
    if data.startswith(b"\x30"):
        valid = certificate(data)
    else:
        remaining = data.strip(SPACE)
        valid = False
        while remaining:
            block = PEM.match(remaining)
            if not block:
                return None
            encoded = block[1].replace(b"\r", b"").replace(b"\n", b"")
            try:
                der = base64.b64decode(encoded, validate=True)
            except binascii.Error:
                return None
            if base64.b64encode(der) != encoded or not certificate(der):
                return None
            valid = True
            remaining = remaining[block.end():].lstrip(SPACE)
    return hashlib.sha256(data).hexdigest() if valid else None


def link_target(path, value):
    if not value or "\\" in value or "\x00" in value or value.endswith("/"):
        return None
    parts = [] if value.startswith("/") else list(PurePosixPath(path).parts[:-1])
    descended = False
    for part in value.split("/"):
        if part in ("", "."):
            continue
        if part == "..":
            # Never collapse a component whose filesystem meaning could be a link.
            if descended or not parts:
                return None
            parts.pop()
        else:
            descended = True
            parts.append(part)
    target = "/".join(parts)
    return target if trust_path(target) else None


class TrustStore:
    def __init__(self):
        self.nodes = {}
        self.dirs = set()
        self.ambiguous = set()

    def observe(self, member, location, physical=None):
        path = location["path"]
        name = PurePosixPath(path)
        if name.name.startswith(".wh."):
            hidden = name.parent if name.name == ".wh..wh..opq" else name.parent / name.name[4:]
            self.ambiguous.add(str(hidden))
            return
        if not (trust_path(path) or path in ROOTS or path in ANCESTORS):
            return
        if any("overlay." in key for key in member.pax_headers):
            self.ambiguous.add(path)
        if path in self.nodes or (path in self.dirs and not member.isdir()):
            self.ambiguous.add(path)
        if member.isdir():
            self.dirs.add(path)
            return
        layer = location["layer"]
        if physical is not None and trust_path(path):
            self.nodes[path] = (layer, certificate_digest(physical), None)
        elif member.issym() and trust_path(path):
            self.nodes[path] = (layer, None, link_target(path, member.linkname))
        else:
            self.nodes[path] = (layer, None, None)

    def resolve(self, path, layer):
        seen = set()
        while path and path not in seen and len(seen) < 40:
            seen.add(path)
            if path in self.ambiguous or any(str(p) in self.ambiguous or str(p) in self.nodes
                                             for p in PurePosixPath(path).parents):
                return None
            node = self.nodes.get(path)
            if not node or node[0] > layer:
                return None
            if node[1]:
                return node[1]
            path = node[2]
        return None

    def classify(self, findings, image):
        for row in findings:
            if (row["scope"] != "layer" or row["image"] != image or "member" in row
                    or row["rule"] != "path.key-or-certificate" or not trust_path(row["path"])):
                continue
            digest = self.resolve(row["path"], row["layer"])
            if digest:
                row["public_certificate_sha256"] = digest
