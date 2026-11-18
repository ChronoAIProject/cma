"""Exact payload, package, container and public-certificate proof contracts."""

import gzip
import io
import zipfile
from functools import lru_cache

from .arch_vectors import fields
from .certificates import PEM, certificate_proof
from .common import ScanError, json_bytes
from .key_vectors import sha
from .marker_policy import identity as marker_identity
from .marker_policy import metadata_paths
from .marker_policy import policy as marker_policy
from .marker_policy import prove as prove_marker
from .policy import CREDENTIAL_URL, PRIVATE


def markers():
    return {marker_identity(e): e for e in marker_policy()}


def required_paths(entry):
    result = set()
    for proof in entry["provenance"]:
        if proof["kind"] == "pacman":
            result.update((proof["status"], proof["listing"]))
        elif proof["kind"] in ("npm", "metadata"):
            result.add(proof["path"])
    if entry["marker"]:
        result.update(metadata_paths(markers()[entry["marker"]]))
    outer = entry["outer"]
    if outer["type"] == "symlink":
        result.add(outer["target"])
        result.update(link["path"] for link in outer["links"])
    return result


def provenance_matches(state, entry, config):
    for proof in entry["provenance"]:
        kind = proof["kind"]
        if kind == "pacman":
            if any(sha(state.get(proof[k], b"")) != proof[k + "_sha256"] for k in ("status", "listing")):
                return False
            desc = fields(state.get(proof["status"], b""))
            listing = fields(state.get(proof["listing"], b""))
            if (
                desc is None
                or listing is None
                or any(
                    desc.get(k) != [proof[v]]
                    for k, v in (("%NAME%", "name"), ("%VERSION%", "version"), ("%ARCH%", "architecture"))
                )
            ):
                return False
            if proof.get("owned") and listing.get("%FILES%", []).count(proof["owned"]) != 1:
                return False
        elif kind in ("npm", "metadata"):
            data = state.get(proof["path"], b"")
            if sha(data) != proof["sha256"]:
                return False
            if kind == "npm":
                try:
                    parsed = json_bytes(data)
                except ScanError:
                    return False
                if not isinstance(parsed, dict) or any(
                    parsed.get(k) != proof[k] for k in ("name", "version")
                ):
                    return False
        elif kind == "omarchy":
            labels = config.get("config", {}).get("Labels", {})
            if labels.get("dev.chrono.cma.omarchy-revision") != proof["version"]:
                return False
        elif kind == "release":
            approved = markers().get(entry["marker"])
            if approved is None or any(approved["package"][k] != proof[k] for k in ("name", "version")):
                return False
        else:
            return False
    return True


def payload(data, entry):
    if entry["member"] is None:
        return data
    member = entry["path"].split("!", 1)[1]
    index = entry["member"]
    # Only the explicitly reviewed single-container formats are eligible.
    if "/" in index:
        return None
    if member == "decoded" and index == "0":
        try:
            with gzip.GzipFile(fileobj=io.BytesIO(data)) as stream:
                return stream.read(entry["payload"]["bytes"] + 1)
        except (OSError, EOFError):
            return None
    try:
        with zipfile.ZipFile(io.BytesIO(data)) as archive:
            entries = archive.infolist()
            if len({e.filename for e in entries}) != len(entries) or int(index) >= len(entries):
                return None
            selected = entries[int(index)]
            if (
                selected.filename != member
                or selected.flag_bits & 1
                or selected.file_size != entry["payload"]["bytes"]
            ):
                return None
            with archive.open(selected) as stream:
                return stream.read(selected.file_size + 1)
    except (OSError, ValueError, zipfile.BadZipFile, RuntimeError):
        return None


@lru_cache(maxsize=64)
def certificate_payload(data):
    # Comments are accepted only in an independently hash-bound certificate file.
    clean = b"\n".join(line for line in data.splitlines() if not line.lstrip().startswith(b"#"))
    proof = certificate_proof(clean)
    return proof, [sha(m[0]) for m in PEM.finditer(data)]


def certificate_matches(data, expected):
    if expected["empty"]:
        return data == b"" and expected["der_sha256"] == [] and expected["pem_sha256"] == []
    proof, blocks = certificate_payload(data)
    return (
        proof is not None
        and proof["certificates"] == expected["der_sha256"]
        and blocks == expected["pem_sha256"]
    )


def prove(data, metadata, state, links, entry, config):
    if not provenance_matches(state, entry, config):
        return False
    outer = entry["outer"]
    if outer["type"] == "symlink":
        if any(sha(links.get(link["path"], "").encode()) != link["sha256"] for link in outer["links"]):
            return False
        data = state.get(outer["target"], b"missing")
        if sha(data) != outer["target_sha256"] or len(data) != outer["target_bytes"]:
            return False
        return certificate_matches(data, entry["certificate"])
    if sha(data) != outer["sha256"] or len(data) != outer["bytes"]:
        return False
    if entry["marker"]:
        marker = markers().get(entry["marker"])
        if marker is None or prove_marker(data, state, marker) is None:
            return False
    decoded = payload(data, entry)
    expected = entry["payload"]
    if decoded is None or sha(decoded) != expected["sha256"] or len(decoded) != expected["bytes"]:
        return False
    if entry["certificate"] is not None and not certificate_matches(decoded, entry["certificate"]):
        return False
    match = entry["match"]
    if "line" in match:
        if not any(
            n == match["line"] and sha(line) == match["line_sha256"]
            for n, line in enumerate(io.BytesIO(decoded), 1)
        ):
            return False
    if "contexts" in match:
        pattern = PRIVATE if match["rule"] == "content.private-key-header" else CREDENTIAL_URL
        actual = [
            {
                "offset": m.start(),
                "bytes": len(m[0]),
                "sha256": sha(m[0]),
                "context_sha256": sha(decoded[max(0, m.start() - 64) : m.end() + 64]),
            }
            for m in pattern.finditer(decoded)
        ]
        if actual != match["contexts"]:
            return False
    return True
