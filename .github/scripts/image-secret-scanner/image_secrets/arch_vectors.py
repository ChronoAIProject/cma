"""Exact Arch package evidence for the two reviewed Omarchy vector files."""

import json
import re
import tarfile

from .common import ScanError, archive_path, json_bytes
from .key_vectors import LIMIT, registry_for, sha, vector_proof, verify_proofs
from .vector_paths import VectorPaths

COMPONENT = "runtime-omarchy"
ROOT = "var/lib/pacman/local/gnutls-3.8.13-3"
PATHS = {"usr/bin/gnutls-cli": 4, "usr/lib/libgnutls.so.30.42.0": 12}
HASHES = "usr/share/cma-omarchy/package-sha256.txt"
ARCHIVE = "/var/cache/pacman/pkg/gnutls-3.8.13-3-x86_64.pkg.tar.zst"


def policy():
    value = json_bytes(registry_for(COMPONENT).read_bytes())
    try:
        package, metadata, files = value["package"], value["metadata"], value["files"]
        valid = (
            set(value) == {"schema", "component", "upstream", "package", "metadata", "files"}
            and value["schema"] == "cma-arch-upstream-self-tests-v1"
            and value["component"] == COMPONENT
            and package["name"] == "gnutls"
            and package["version"] == "3.8.13-3"
            and package["architecture"] == "x86_64"
            and metadata["root"] == ROOT
            and metadata["package_hashes"] == HASHES
            and metadata["archive_path"] == ARCHIVE
            and len(files) == 2
            and {item["path"] for item in files} == set(PATHS)
        )
        hashes = [
            package["archive_sha256"],
            metadata["mtree_sha256"],
            value["upstream"]["archive_sha256"],
            *value["upstream"]["files"].values(),
        ]
        for item in files:
            valid &= (
                set(item) == {"path", "sha256", "bytes", "blocks"}
                and type(item["bytes"]) is int
                and 0 < item["bytes"] <= LIMIT
                and len(item["blocks"]) == PATHS[item["path"]]
            )
            hashes.append(item["sha256"])
            last = -1
            for block in item["blocks"]:
                valid &= (
                    set(block) == {"offset", "length", "sha256"}
                    and type(block["offset"]) is int
                    and block["offset"] > last
                    and type(block["length"]) is int
                    and 0 < block["length"] <= 32768
                    and block["offset"] + block["length"] <= item["bytes"]
                )
                last = block["offset"] + block["length"] - 1
                hashes.append(block["sha256"])
        valid &= all(isinstance(digest, str) and re.fullmatch(r"[a-f0-9]{64}", digest) for digest in hashes)
    except (KeyError, TypeError, ValueError):
        valid = False
    if not valid:
        raise ScanError("invalid-arch-vector-policy")
    return value


def fields(data):
    """Pacman local records use unique, percent-delimited fields."""
    try:
        result = {}
        for section in data.decode("utf-8").strip().split("\n\n"):
            lines = section.splitlines()
            if not lines or not re.fullmatch(r"%[A-Z0-9_]+%", lines[0]) or lines[0] in result:
                return None
            result[lines[0]] = lines[1:]
        return result
    except UnicodeError:
        return None


def evidence_matches(state, expected):
    description = fields(state.get(ROOT + "/desc", b""))
    owned = fields(state.get(ROOT + "/files", b""))
    if description is None or owned is None:
        return False
    package = expected["package"]
    for field, key in (("%NAME%", "name"), ("%VERSION%", "version"), ("%ARCH%", "architecture")):
        if description.get(field) != [package[key]]:
            return False
    names = owned.get("%FILES%", [])
    if any(names.count(item["path"]) != 1 for item in expected["files"]):
        return False
    record = (package["archive_sha256"] + "  " + ARCHIVE).encode()
    return (
        sha(state.get(ROOT + "/mtree", b"")) == expected["metadata"]["mtree_sha256"]
        and state.get(HASHES, b"").splitlines().count(record) == 1
    )


def provenance_digest(expected):
    # Bind exactly the relevant package record; unrelated installed packages do
    # not become an allowance. The official whole archive was independently pinned.
    value = {
        "package": expected["package"],
        "metadata": expected["metadata"],
        "files": [{key: item[key] for key in ("path", "sha256", "bytes")} for item in expected["files"]],
    }
    return sha(json.dumps(value, sort_keys=True, separators=(",", ":")).encode())


def reviewed(result):
    expected = policy()
    digest = sha(registry_for(COMPONENT).read_bytes())
    return verify_proofs(
        result,
        digest,
        [
            {
                "library": item,
                "blocks": item["blocks"],
                "package_sha256": expected["package"]["archive_sha256"],
                "provenance_sha256": provenance_digest(expected),
            }
            for item in expected["files"]
        ],
    )


class ArchVectorState:
    def __init__(self):
        self.expected = policy()
        self.state = {}
        self.paths = VectorPaths([*PATHS, ROOT + "/desc", ROOT + "/files", ROOT + "/mtree", HASHES])
        self.conflicting = False

    def layer(self, path, image, layer, findings):
        files = {item["path"]: item for item in self.expected["files"]}
        required = {ROOT + suffix for suffix in ("/desc", "/files", "/mtree")} | {HASHES}
        seen, candidates, duplicate = set(), [], False
        with tarfile.open(path, "r:") as archive:
            for index, entry in enumerate(archive):
                name = archive_path(entry.name)
                self.paths.observe(name, entry)
                if any(part.startswith(".wh.") for part in name.split("/")):
                    self.state.clear()
                    self.conflicting = False
                if (
                    name.startswith("var/lib/pacman/local/gnutls-")
                    and name.endswith("/desc")
                    and name != ROOT + "/desc"
                ):
                    self.conflicting = True
                if name not in required and name not in files:
                    continue
                duplicate |= name in seen
                seen.add(name)
                data = b""
                if entry.isfile() and entry.size <= LIMIT:
                    with archive.extractfile(entry) as stream:
                        data = stream.read(LIMIT + 1)
                if name in required:
                    self.state[name] = data
                if name in files:
                    candidates.append((index, name, data))
        if duplicate:
            self.state.clear()
        if (
            duplicate
            or self.conflicting
            or self.paths.redirected
            or not evidence_matches(self.state, self.expected)
        ):
            return
        for index, name, data in candidates:
            item = files[name]
            proof = vector_proof(
                data,
                item,
                item["blocks"],
                self.expected["package"]["archive_sha256"],
                sha(registry_for(COMPONENT).read_bytes()),
                provenance_digest(self.expected),
            )
            if proof is not None:
                findings.upstream_vectors[(image, layer, index, name)] = proof
