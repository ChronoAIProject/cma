"""Prove certificate-only material against one image's archive namespace."""

import base64
import binascii
import hashlib
import re
from pathlib import PurePosixPath

from .common import digest_bytes
from .key_material import decode, frame, has_binary_key

ROOTS = ("etc/ssl/certs", "usr/share/ca-certificates", "usr/local/share/ca-certificates",
         "etc/ca-certificates", "etc/pki")
BUNDLES = ("etc/ssl/cert.pem", "usr/lib/ssl/cert.pem")
ANCESTORS = {str(p) for name in (*ROOTS, *BUNDLES) for p in PurePosixPath(name).parents}
PEM = re.compile(rb"-----BEGIN CERTIFICATE-----[ \t]*\r?\n([A-Za-z0-9+/=\r\n]+)-----END CERTIFICATE-----")
COMMENT_OR_BLANK = re.compile(rb"(?:[ \t]*(?:\#[^\r\n]*)?\r?\n)*[ \t]*(?:\#[^\r\n]*\Z)?")
SPACE = b" \r\n\t"
MAX_CERT_FILE = 4 * 1024 * 1024


def trust_path(path):
    return path in BUNDLES or any(path.startswith(root + "/") for root in ROOTS)


def certificate(data, inspect_der):
    outer = frame(data, 0, len(data))
    if not outer or outer[0] != 48 or outer[2] != len(data):
        return False
    parsed = decode(["x509", "-inform", "DER", "-outform", "DER"], data)
    if parsed.returncode or parsed.stdout != data:
        return False
    inspect_der(data)
    return not has_binary_key(data) and b"PRIVATE KEY-----" not in data


def certificate_digest(path, inspect_der, *, pem_bundle_limit=None):
    size = path.stat().st_size
    if not 0 < size <= MAX_CERT_FILE:
        return None
    data = path.read_bytes()
    if data.startswith(b"\x30") and pem_bundle_limit is None:
        valid = certificate(data, inspect_der)
    else:
        count = 0
        remaining = (data.strip(SPACE) if pem_bundle_limit is None
                     else data[COMMENT_OR_BLANK.match(data).end():])
        valid = False
        while remaining:
            block = PEM.match(remaining)
            if not block or (pem_bundle_limit is not None and count >= pem_bundle_limit):
                return None
            encoded = block[1].replace(b"\r", b"").replace(b"\n", b"")
            try:
                der = base64.b64decode(encoded, validate=True)
            except binascii.Error:
                return None
            if base64.b64encode(der) != encoded or not certificate(der, inspect_der):
                return None
            valid = True
            count += 1
            remaining = remaining[block.end():]
            if pem_bundle_limit is None:
                remaining = remaining.lstrip(SPACE)
            else:
                # Consume the end of this physical line before allowing comments.
                end = re.match(rb"[ \t]*(?:\r?\n|\Z)", remaining)
                if end is None:
                    return None
                remaining = remaining[COMMENT_OR_BLANK.match(remaining, end.end()).end():]
    return hashlib.sha256(data).hexdigest() if valid else None


def certificate_inventory(physical, inspect_der, count, *, comments=False):
    hashes = []

    def observe(der):
        inspect_der(der)
        hashes.append(digest_bytes(der))

    member_digest = certificate_digest(physical, observe, pem_bundle_limit=count if comments else None)
    if member_digest and len(hashes) == len(set(hashes)) == count:
        return {"member_sha256": member_digest, "der_sha256s": sorted(hashes)}
    return None


def link_target(path, value):
    if not value or "\\" in value or "\x00" in value or value.rsplit("/", 1)[-1] in ("", ".", ".."):
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
    def __init__(self, tracked=()):
        self.nodes = {}
        self.dirs = set()
        self.ambiguous = set()
        # Reviewed paths outside the OS trust roots, such as a package-owned
        # certificate, get the same replacement and ancestor accounting.
        self.tracked = {str(p) for path in tracked for p in (PurePosixPath(path), *PurePosixPath(path).parents)}

    def observe(self, member, location, physical, inspect_der):
        path = location["path"]
        name = PurePosixPath(path)
        if name.name.startswith(".wh."):
            hidden = name.parent if name.name == ".wh..wh..opq" else name.parent / name.name[4:]
            self.ambiguous.add(str(hidden))
            return
        if not (trust_path(path) or path in ROOTS or path in ANCESTORS or path in self.tracked):
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
            self.nodes[path] = (layer, certificate_digest(physical, inspect_der), None)
        elif member.issym() and trust_path(path):
            self.nodes[path] = (layer, None, link_target(path, member.linkname))
        else:
            self.nodes[path] = (layer, None, None)

    def resolve(self, path, layer):
        seen = set()
        while path and path not in seen and len(seen) < 40:
            seen.add(path)
            if self.unresolved(path):
                return None
            node = self.nodes.get(path)
            if not node or node[0] > layer:
                return None
            if node[1]:
                return node[1]
            path = node[2]
        return None

    def unresolved(self, path):
        return path in self.ambiguous or any(
            str(parent) in self.ambiguous or str(parent) in self.nodes
            for parent in PurePosixPath(path).parents
        )

    def final_entry(self, path, layer):
        # The layer's entry is the only one at this path, and no entry hides,
        # replaces or overlays it or an ancestor in any layer of the image.
        return not self.unresolved(path) and self.nodes.get(path) == (layer, None, None)

    def direct_alias(self, path, target, layer):
        if self.unresolved(path) or self.unresolved(target) or self.nodes.get(path) != (layer, None, target):
            return False
        node = self.nodes.get(target)
        return node is not None and node[0] == layer and node[2] is None

    def classify(self, findings, image):
        # A credential anywhere in a target or its decoded views also disqualifies
        # every alias. This decision runs only after all layer detectors finish.
        self.ambiguous.update(row["path"] for row in findings
                              if row["scope"] == "layer" and row["image"] == image
                              and row["rule"] not in {"path.key-or-certificate", "gitleaks.image-high-entropy"})
        for row in findings:
            if (row["scope"] != "layer" or row["image"] != image or "member" in row
                    or row["rule"] != "path.key-or-certificate" or not trust_path(row["path"])):
                continue
            digest = self.resolve(row["path"], row["layer"])
            if digest:
                row["public_certificate_sha256"] = digest
