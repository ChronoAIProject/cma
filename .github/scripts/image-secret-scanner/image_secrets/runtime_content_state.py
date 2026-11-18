"""Prove exact content against package state without trusting the final overlay."""

import json
import tarfile

from .common import ScanError, archive_path
from .runtime_content import LIMIT, REGISTRY, finding_matches, identity, metadata_paths, policy, prove, sha
from .vector_paths import VectorPaths


class RuntimeContentState:
    def __init__(self, component, architecture):
        self.entries = [
            e for e in policy() if component in e["components"] and architecture == e["architecture"]
        ]
        self.required = set().union(*(metadata_paths(e) for e in self.entries))
        self.files = {e["path"] for e in self.entries}
        self.bounds = {
            name: max(e["bytes"] for e in self.entries if e["path"] == name) for name in self.files
        }
        self.paths = VectorPaths(self.required | self.files)
        self.state = {}

    def layer(self, path, image, layer, findings):
        seen, ambiguous, candidates = set(), set(), []
        with tarfile.open(path, "r:") as archive:
            for index, entry in enumerate(archive):
                name = archive_path(entry.name)
                self.paths.observe(name, entry)
                if any(part.startswith(".wh.") for part in name.split("/")):
                    self.state.clear()
                if name not in self.required | self.files:
                    continue
                if name in seen:
                    ambiguous.add(name)
                seen.add(name)
                data = b""
                bound = max(self.bounds.get(name, 0), 8 * 1024 * 1024 if name in self.required else 0)
                if entry.isfile() and entry.size <= bound:
                    with archive.extractfile(entry) as stream:
                        data = stream.read(bound + 1)
                if name in self.required:
                    self.state[name] = data
                if name in self.files:
                    metadata = json.dumps(
                        {"path": name, "link": entry.linkname, "pax": entry.pax_headers}, ensure_ascii=False
                    ).encode()
                    candidates.append((index, name, data, metadata))
                if sum(map(len, self.state.values())) + sum(len(c[2]) for c in candidates) > 2 * LIMIT:
                    raise ScanError("runtime-content-state-limit")
        if sum(map(len, self.state.values())) + sum(len(c[2]) for c in candidates) > 2 * LIMIT:
            raise ScanError("runtime-content-state-limit")
        if ambiguous or self.paths.redirected:
            self.state.clear()
            return
        by_location = {}
        for finding in findings.items:
            if finding.get("image") == image and finding.get("layer") == layer:
                by_location.setdefault((finding.get("entry"), finding["path"]), []).append(finding)
        for index, name, data, metadata in candidates:
            for expected in self.entries:
                if name != expected["path"] or not prove(data, metadata, self.state, expected):
                    continue
                for original in by_location.get((index, name), []):
                    safe = findings.safe_items([original])[0]
                    if finding_matches(safe, expected):
                        findings.reviewed_runtime_content.append(
                            {
                                "finding_id": safe["finding_id"],
                                "entry_sha256": identity(expected),
                                "policy_sha256": sha(REGISTRY.read_bytes()),
                                "file_sha256": expected["file_sha256"],
                                "detector_input_sha256": expected["detector_input_sha256"],
                            }
                        )
