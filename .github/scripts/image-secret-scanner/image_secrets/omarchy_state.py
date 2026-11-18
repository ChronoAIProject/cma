"""Retain exact reviewed evidence per layer; later layers cannot authorize earlier bytes."""

import json
import tarfile

from .arch_vectors import fields
from .common import ScanError, archive_path
from .key_vectors import sha
from .omarchy_policy import REGISTRY, finding_matches, identity, policy
from .omarchy_proofs import prove, required_paths
from .vector_paths import VectorPaths

LIMIT = 1024 * 1024 * 1024


class OmarchyState:
    def __init__(self, component, config):
        self.entries = [
            e
            for e in policy()
            if e["component"] == component and e["architecture"] == config.get("architecture")
        ]
        self.config = config
        self.files = {e["outer"]["path"] for e in self.entries}
        self.required = set().union(*(required_paths(e) for e in self.entries))
        self.paths = VectorPaths(self.files | self.required)
        self.state, self.links = {}, {}
        self.conflicting = False
        self.package_roots = {
            "var/lib/pacman/local/" + p["name"] + "-": p["status"]
            for e in self.entries
            for p in e["provenance"]
            if p["kind"] == "pacman"
        }
        self.mime = any(e["marker"] and e["outer"]["path"] == "usr/share/mime/magic" for e in self.entries)

    def layer(self, path, image, layer, findings):
        if not self.entries:
            return
        candidates, seen, ambiguous = [], set(), False
        observed_links = {
            link["path"]
            for e in self.entries
            if e["outer"]["type"] == "symlink"
            for link in e["outer"]["links"]
        }
        with tarfile.open(path, "r:") as archive:
            for index, entry in enumerate(archive):
                name = archive_path(entry.name)
                self.paths.observe(name, entry)
                if any(part.startswith(".wh.") for part in name.split("/")):
                    self.state.clear()
                    self.links.clear()
                if name.endswith("/desc") and entry.isfile() and entry.size <= 1024 * 1024:
                    possible = [
                        (prefix, expected)
                        for prefix, expected in self.package_roots.items()
                        if name.startswith(prefix) and name != expected
                    ]
                    if possible:
                        with archive.extractfile(entry) as stream:
                            description = fields(stream.read())
                        if description is None or any(
                            description.get("%NAME%") == [prefix.removeprefix("var/lib/pacman/local/")[:-1]]
                            for prefix, _expected in possible
                        ):
                            self.conflicting = True
                if self.mime and name.startswith("usr/share/mime/packages/") and not entry.isdir():
                    self.state[name] = b""
                if name not in self.files | self.required:
                    continue
                if name in seen:
                    ambiguous = True
                seen.add(name)
                data = b""
                if entry.isfile() and entry.size <= LIMIT:
                    with archive.extractfile(entry) as stream:
                        data = stream.read(LIMIT + 1)
                if name in observed_links:
                    self.links[name] = entry.linkname if entry.issym() else ""
                elif not entry.isfile():
                    ambiguous = True
                self.state[name] = data
                if name in self.files:
                    metadata = json.dumps(
                        {"path": name, "link": entry.linkname, "pax": entry.pax_headers}, ensure_ascii=False
                    ).encode()
                    candidates.append((index, name, metadata))
                if sum(map(len, self.state.values())) > LIMIT:
                    raise ScanError("omarchy-artifact-state-limit")
        if ambiguous or self.paths.redirected or self.conflicting:
            self.state.clear()
            return
        by_location = {}
        for item in findings.items:
            if item.get("image") == image and item.get("layer") == layer:
                by_location.setdefault(item.get("entry"), []).append(item)
        for index, name, metadata in candidates:
            for entry in self.entries:
                if name != entry["outer"]["path"]:
                    continue
                if name not in self.state or not prove(
                    self.state[name], metadata, self.state, self.links, entry, self.config
                ):
                    continue
                for original in by_location.get(index, []):
                    safe = findings.safe_items([original])[0]
                    if finding_matches(safe, entry):
                        findings.reviewed_omarchy_artifacts.append(
                            {
                                "finding_id": safe["finding_id"],
                                "entry_sha256": identity(entry),
                                "policy_sha256": sha(REGISTRY.read_bytes()),
                            }
                        )
