"""Prove a pinned ELF private-key detector span whose interior is compiler read-only data."""

import re

from . import elf_constants, key_marker_view, pooled_literals, reviewed_literals
from .binary_view import TEXT_BYTES
from .common import digest_bytes
from .gate import valid_digest
from .package_evidence import fields, image_binding, require

MODE = "elf-rodata-marker-gap-v1"
MAX_SPAN = 32 * 1024
# The interior must not hold anything shaped like PEM key material.
ARMOR_RUN = re.compile(rb"[A-Za-z0-9+/=]{40,}")
HEADERS = re.compile(rb"proc-type:|dek-info:", re.I)
PRIVATE_LABEL = re.compile(rb"-----BEGIN[ A-Z0-9_-]{0,100}PRIVATE KEY(?: BLOCK)?-----", re.I)
# Reviewed exceptions, each named by a pinned match: compiler __FILE__ build
# paths under one exact prefix, and bare PEM header field names used as
# search needles. Both are masked to NUL only after their exact shape holds.
BUILD_PATH = rb"[A-Za-z0-9_/]+\.(?:cxx|h)"
FIELD_NAME = re.compile(rb"(?:proc-type|dek-info):", re.I)
NATIVE_STRING = re.compile(rb"[^\0]+")
RSAAES = ("common/rfb/SSecurityRSAAES.cxx", "8ca8fec6fee0c6bcefcf730ab8af768c26cf2c2967afc34d4e280d924243f8c9")
TLSKEY = ("src/plugins/tls/shared/qtlskey_base.cpp", "05f704c056c9a14d1244011ebd2b0054278d51bb80caaf1745f220e50da71077")
OPENSSL_KEY = ("src/plugins/tls/openssl/qtlskey_openssl.cpp",
               "e8dcafdbc348c1ff7be2be8bba5067db432b80ef720cc44c0716fe20564ba4b7")
TIGERVNC_BUILD = b"/usr/src/debug/tigervnc/tigervnc/"


def span(offset, size, sha256):
    return {"offset": offset, "size": size, "sha256": sha256}


def label(offset, size, source, line, sha256):
    path, proof = source
    return {"offset": offset, "size": size, "sha256": sha256, "source": path, "line": line, "proof_sha256": proof}


# Reviewed detector spans that run from a published PEM label across ordinary
# compiler read-only data to the next label. Each interior is pinned by
# digest rather than accounted for by declarations, and is re-checked for
# key-shaped content at scan time.
PINS = {
    ("tigervnc", "usr/bin/x0vncserver"): {
        "version": "1.16.2-5", "architecture": "x86_64",
        "artifact_sha256": "09835681f48df886fe52037f3da9d2ed0327b70b4be77c060248abf213f1b550",
        "file_sha256": "eac0e6cf19b87f7739f4f98d15c959fd61bf1340b6e5e045685a7638c46ef503",
        "matches": (
            {
                "original": span(426769, 19574,
                    "0db517c290d281d2fe9d8e0b8723070fa5d619ad512fae10d148e04dbeb47f9b"),
                "detector_sha256": "ffee22df7b83d5f72d1171db68c1c2d616cfb66762763639d04a66a71917088b",
                "begin": label(426769, 27, RSAAES, 202,
                    "3021d90eb9437b2d8f30e8363695c4418b5e5f1870801b5c317e9398ee0f572d"),
                "terminator": label(446312, 31, RSAAES, 196,
                    "8bcac7908eb950419537b91e19adc83ce2c9cbfdacf4f81157fdadfec11f7017"),
                "interior_sha256": "02c707fdf7cef2c1d2ee125b6ad29891a883de0d537415543bbda546aa7dae90",
                "build_paths": {"prefix": TIGERVNC_BUILD, "count": 17},
            },
        ),
    },
    ("tigervnc", "usr/lib/xorg/modules/extensions/libvnc.so"): {
        "version": "1.16.2-5", "architecture": "x86_64",
        "artifact_sha256": "09835681f48df886fe52037f3da9d2ed0327b70b4be77c060248abf213f1b550",
        "file_sha256": "f7a76dee57dba13ab8954d3bccc645b346d978bea6769bc2bc5cb0f75d78d92c",
        "matches": (
            {
                "original": span(556633, 18278,
                    "3343a327382e1e6301b72e387d792c86ffeb2e7150d549989a2614d911380e89"),
                "detector_sha256": "d5d1f1bebbe801b09e1e6add29b04ed7d1ea4ebcbf1774df7d1e00968e75e926",
                "begin": label(556633, 27, RSAAES, 202,
                    "3021d90eb9437b2d8f30e8363695c4418b5e5f1870801b5c317e9398ee0f572d"),
                "terminator": label(574880, 31, RSAAES, 196,
                    "8bcac7908eb950419537b91e19adc83ce2c9cbfdacf4f81157fdadfec11f7017"),
                "interior_sha256": "676d5128236b3ad8a87bf6089a9cdfef4811a22806e04fedafe950a5800229be",
                "build_paths": {"prefix": TIGERVNC_BUILD, "count": 12},
            },
        ),
    },
    ("qt6-base", "usr/lib/qt6/plugins/tls/libqopensslbackend.so"): {
        "version": "6.12.0-3", "architecture": "x86_64",
        "artifact_sha256": "dea8a8c45a7929af115a2ba96c60dff927425f8c65432cf5d1027c79c8aa98f0",
        "file_sha256": "61c88ac6c5138736c2ff29a916df76853c63cc594b40ff2d1a5cc7736aaa03dd",
        "matches": (
            {
                "original": span(234176, 1971,
                    "fe2a88588403885b9da04929b3bd68a9d53be3401320e50500edf664f16d092f"),
                "detector_sha256": "563af6bfed9960b32ac0918a1dd9b5b2c9624f26fe917c8fef77ad88f56e091b",
                "begin": label(234176, 27, TLSKEY, 53,
                    "3021d90eb9437b2d8f30e8363695c4418b5e5f1870801b5c317e9398ee0f572d"),
                "terminator": label(236112, 35, TLSKEY, 59,
                    "21830902564df162c79ae42997a1dca1be54720d67bfafab4b41470ab533e194"),
                "interior_sha256": "dff1f1428461fc0d47cf70ff5acf1c7d468b2d98ed5df897d428acdc614cef5a",
                "field_names": (label(234641, 10, OPENSSL_KEY, 132,
                    "b02760e1e106ff44c6953f17efbffa7b88ea70c2e7fee3289619c4d172847269"),),
            },
            {
                "original": span(236152, 3773,
                    "395b34237f558c0478e21c65f347ccace92be4ba22fbedd9ca9cbab8fc550a7e"),
                "detector_sha256": "42f497c102b391317ca8483ae15d03280c92d3dfa0959747f0cb01f012436bf1",
                "begin": label(236152, 37, TLSKEY, 52,
                    "dec643eed02d96980d7003df682d77b28a23f90a6d2a0961886c8953c2e8cb4e"),
                "terminator": label(239888, 37, TLSKEY, 52,
                    "dec643eed02d96980d7003df682d77b28a23f90a6d2a0961886c8953c2e8cb4e"),
                "interior_sha256": "8facf06ff5da69b6f12c712c4a0881e86417528ad46b976943c29762101b7058",
                "terminator_pool": pooled_literals.POOL,
            },
        ),
    },
}
BY_FILE = {pin["file_sha256"]: pin for pin in PINS.values()}


def expected(match):
    begin, terminator = match["begin"], match["terminator"]
    start = begin["offset"] + begin["size"]
    return {"original": match["original"], "detector": dict(match["original"], sha256=match["detector_sha256"]),
            "declarations": [begin, terminator],
            "interior": span(start, terminator["offset"] - start, match["interior_sha256"])}


def validate(row, package, encoding, *, portable=False):
    proof = row["elf_nul_key_view"]
    image_binding(proof, {"encoding", "marker_gap", "matches"}, portable=portable)
    pin = PINS.get((package["name"], row["path"]))
    require(pin is not None and proof["encoding"] == encoding and proof["marker_gap"] == MODE
            and row["component"] == key_marker_view.SCOPE and row["platform"] == "linux/amd64"
            and row["member"] is None and package["authority_type"] == "arch-signature"
            and all(package[key] == pin[key] for key in ("version", "architecture", "artifact_sha256"))
            and row["file_sha256"] == pin["file_sha256"]
            and isinstance(proof["matches"], list) and len(proof["matches"]) == len(pin["matches"]))
    names = {"original", "detector", "declarations", "interior"}
    candidates = {}
    for item, match in zip(proof["matches"], pin["matches"]):
        fields(item, names if portable else names | {"finding_id"})
        require(portable or valid_digest(item["finding_id"]))
        reference = expected(match)
        start, stop = reviewed_literals.extent(reference["original"])
        begin, terminator = reference["declarations"]
        require({key: item[key] for key in names} == reference
                and begin["offset"] == start and terminator["offset"] + terminator["size"] == stop
                and reference["interior"]["size"] > 0 and stop - start <= MAX_SPAN
                and reference["detector"]["sha256"] not in candidates)
        candidates[reference["detector"]["sha256"]] = item
    return candidates


def complete_label(data, part, frame):
    """A printable PEM label that is a whole native string, optionally newline-terminated."""
    left, right = reviewed_literals.extent(part)
    body = data[left:right]
    return (0 < left and data[left - 1] == 0 and (data[right:right + 1] == b"\0" or data[right:right + 2] == b"\n\0")
            and all(32 <= value < 127 for value in body) and digest_bytes(body) == part["sha256"]
            and key_marker_view.FRAME[frame].fullmatch(body) is not None)


def pooled_label(data, part, pool):
    """The terminating label starts the pinned read-only vector constant pool."""
    left, right = reviewed_literals.extent(part)
    body = data[left:right]
    return (0 < left and data[left - 1] == 0 and all(32 <= value < 127 for value in body)
            and digest_bytes(body) == part["sha256"] and key_marker_view.FRAME["end"].fullmatch(body) is not None
            and pooled_literals.pooled(data, {"span": span(left, right - left, part["sha256"]), "context": pool}))


def masked_interior(data, left, right, match):
    """Mask only the reviewed public strings that the pin names, or None when they differ."""
    interior = bytearray(data[left:right])
    paths = match.get("build_paths")
    if paths is not None:
        pattern = re.compile(re.escape(paths["prefix"]) + BUILD_PATH)
        found = [run for run in NATIVE_STRING.finditer(interior) if pattern.fullmatch(run[0])]
        if len(found) != paths["count"]:
            return None
        for run in found:
            interior[run.start():run.end()] = bytes(run.end() - run.start())
    for field in match.get("field_names", ()):
        start, stop = reviewed_literals.extent(field)
        body = data[start:stop]
        if (not left < start < stop < right or data[start - 1] != 0 or data[stop] != 0
                or digest_bytes(body) != field["sha256"] or FIELD_NAME.fullmatch(body) is None):
            return None
        interior[start - left:stop - left] = bytes(stop - start)
    return bytes(interior)


def public_interior(interior):
    compact = interior.replace(b"\r", b"").replace(b"\n", b"")
    return not any(pattern.search(view) for pattern in (ARMOR_RUN, HEADERS, PRIVATE_LABEL)
                   for view in (interior, compact))


def verified_span(data, rendered, metadata, item, match, segments):
    start, stop = reviewed_literals.extent(item["original"])
    if stop > len(data) or elf_constants.address_of(segments, start, stop - start, (4,)) is None:
        return False
    physical, detector = data[start:stop], rendered[start:stop]
    if (digest_bytes(physical) != item["original"]["sha256"] or digest_bytes(detector) != item["detector"]["sha256"]
            or data.find(physical) != start or data.find(physical, start + 1) != -1
            or rendered.find(detector) != start or rendered.find(detector, start + 1) != -1
            or physical in metadata or detector in metadata.translate(TEXT_BYTES)):
        return False
    begin, terminator = item["declarations"]
    left, right = reviewed_literals.extent(item["interior"])
    pool = match.get("terminator_pool")
    terminated = (complete_label(data, terminator, "end") if pool is None
                  else pooled_label(data, terminator, pool))
    if (not complete_label(data, begin, "begin") or not terminated
            or digest_bytes(data[left:right]) != item["interior"]["sha256"]):
        return False
    interior = masked_interior(data, left, right, match)
    return interior is not None and public_interior(interior)


def verify(data, rendered, metadata, proof):
    pin = BY_FILE.get(digest_bytes(data))
    if pin is None or len(proof["matches"]) != len(pin["matches"]):
        return {}
    segments = elf_constants.load_segments(data)
    return {item["detector"]["sha256"]: item for item, match in zip(proof["matches"], pin["matches"])
            if expected(match) == {key: item[key] for key in expected(match)}
            and verified_span(data, rendered, metadata, item, match, segments)}
