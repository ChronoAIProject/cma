"""Bind reviewed non-key markers to the package state in their own image layer."""

import tarfile

from .common import ScanError, archive_path
from .marker_policy import LIMIT, metadata_paths, policy, prove
from .vector_paths import VectorPaths


class MarkerState:
    def __init__(self, component, architecture):
        self.entries = [
            entry
            for entry in policy()
            if component in entry["components"] and architecture == entry["architecture"]
        ]
        self.required = set().union(*(metadata_paths(entry) for entry in self.entries))
        self.files = {entry["path"] for entry in self.entries}
        self.paths = VectorPaths(self.required | self.files)
        self.state = {}
        self.package_roots = {
            "var/lib/pacman/local/" + entry["package"]["name"] + "-": entry["metadata"]["status"]
            for entry in self.entries
            if entry["metadata"]["kind"] == "pacman"
        }
        self.conflicting = False
        self.mime_inputs = any(entry.get("structure", {}).get("kind") == "mime" for entry in self.entries)

    def layer(self, path, image, layer, findings):
        seen, ambiguous, candidates = set(), set(), []
        with tarfile.open(path, "r:") as archive:
            for index, entry in enumerate(archive):
                name = archive_path(entry.name)
                self.paths.observe(name, entry)
                if any(part.startswith(".wh.") for part in name.split("/")):
                    self.state.clear()
                if name.endswith("/desc") and any(
                    name.startswith(prefix) and name != exact for prefix, exact in self.package_roots.items()
                ):
                    self.conflicting = True
                if self.mime_inputs and name.startswith("usr/share/mime/packages/") and not entry.isdir():
                    # Record the entire input inventory, including unreviewed
                    # files and links; only exact listed inputs can prove a cache.
                    self.state[name] = b""
                if name not in self.required | self.files:
                    continue
                if name in seen:
                    ambiguous.add(name)
                seen.add(name)
                data = b""
                if entry.isfile() and entry.size <= LIMIT:
                    with archive.extractfile(entry) as stream:
                        data = stream.read(LIMIT + 1)
                if name in self.required:
                    self.state[name] = data
                if name in self.files:
                    candidates.append((index, name, data))
        if sum(map(len, self.state.values())) > LIMIT:
            raise ScanError("reviewed-marker-state-limit")
        if ambiguous or self.paths.redirected or self.conflicting:
            self.state.clear()
            return
        for index, name, data in candidates:
            for expected in self.entries:
                if name == expected["path"]:
                    proof = prove(data, self.state, expected)
                    if proof is not None:
                        findings.reviewed_markers[(image, layer, index, name, None)] = proof
                        structure = expected.get("structure", {})
                        if structure.get("kind") == "ar":
                            member = structure["member"]
                            key = (image, layer, index, name + "!" + member["name"], str(member["index"]))
                            findings.reviewed_markers[key] = dict(proof, member=str(member["index"]))
