"""Locate reviewed default-runtime members in the current physical layers."""

from collections import defaultdict

from . import mime_sources
from .arch_package_ownership import ArchOwnership
from .common import archive_path
from .portable_ownership import STATUS_PATH, check_reserved_inputs, dpkg_identity, hides_status, status_entry


class PortableOccurrences:
    def __init__(self, entries, packages, source, component, trust=None):
        self.entries = entries
        self.packages = packages
        self.source = source
        # Each reviewed container maps its top-level row (None) and member rows by locator.
        self.containers = defaultdict(dict)
        self.by_path = defaultdict(dict)
        self.databases = defaultdict(list)
        self.inputs = set()
        self.status = {}
        self.masked_status = set()
        # A package-owned trust certificate needs the same pacman ownership proof as a package row.
        self.trust_rows = {}
        for path, record in (trust["files"].items() if trust else ()):
            native = record["native"]
            if native is not None:
                self.trust_rows[path, record["file_sha256"]] = {
                    "path": path, "file_sha256": record["file_sha256"], "package_sha256": native["package_sha256"],
                    "ownership": {"mtree_sha256": native["mtree_sha256"]}}
        # Omarchy also carries CMA's pinned producers; only signed rows use pacman.
        signed = (row for row, _ in entries.values() if not self.producer(row))
        self.arch = (ArchOwnership([*signed, *self.trust_rows.values()], packages)
                     if component == "runtime-omarchy" else None)
        self.mime = None
        for (platform, path, sha256, locator), record in entries.items():
            row, _ = record
            self.containers[platform, path, sha256][locator] = record
            self.by_path[platform, path][sha256] = self.containers[platform, path, sha256]
            for name in ("self_test", "distribution_config"):
                if name in row:
                    database = row[name]["database"]
                    self.databases[platform, database["path"]].append((path, database))
            origin = row.get("derived_payload", {})
            if origin.get("kind") == "mime-index":
                self.inputs.add((platform, row["package_member"]["path"][-1]))
                if "sources" in origin:
                    self.mime = self.mime or mime_sources.VisibleSources()
                    self.inputs.update((platform, source["path"]) for source in origin["sources"])

    def select(self, platform, layer, member, budget, stream_digest, coordinate):
        path = archive_path(member.name, posix_tar=True)
        if self.arch is not None and platform == "linux/amd64":
            self.arch.observe(layer, member, path, coordinate, budget)
            if self.mime is not None:
                self.mime.observe(member, path, coordinate)
        elif hides_status(path, member):
            self.masked_status.add(coordinate[:2])
        if self.arch is None and path == STATUS_PATH:
            scope = coordinate[:2]
            status = status_entry(layer, member, budget) if scope not in self.status else None
            self.status[scope] = (coordinate[2], *status) if status is not None else None
            return None
        candidates = self.by_path.get((platform, path), {})
        databases = self.databases.get((platform, path), ())
        if not candidates and not databases and (platform, path) not in self.inputs:
            return None
        if len(candidates) > 1 and member.isfile() and not member.issparse():
            # Two authenticated versions can ship at the same path in different
            # physical layers. Reopen the exact version present in this member.
            budget.reserve(member.size)
            with layer.extractfile(member) as stream:
                candidate = candidates.get(stream_digest(stream, member.size))
        else:
            candidate = next(iter(candidates.values()), None)
        return path, candidate, databases

    def eligible(self, observations):
        matching = defaultdict(list)
        installed = {}
        for coordinate, (platform, _, _, _, (path, sha256, _)) in observations.items():
            installed[(*coordinate[:2], path)] = sha256
            key = (platform, path, sha256)
            if key in self.containers:
                matching[coordinate[0], key].append(coordinate)
        # Any unexplained copy makes the whole group ineligible for review.
        result = {}
        source_checks = {}
        arch_candidates = defaultdict(list)
        generated = []
        for (_, key), coordinates in matching.items():
            # Rows in one container share their ownership fields and occurrence count.
            row, _ = next(iter(self.containers[key].values()))
            if len(coordinates) != row["occurrences"]:
                continue
            for coordinate in coordinates:
                if self.arch is not None and not self.producer(row):
                    if "derived_payload" in row:
                        generated.append((coordinate, row))
                    else:
                        arch_candidates[coordinate[:2]].append((coordinate, row, self.packages[row["package_sha256"]]))
                    continue
                owner = self.owner(row, coordinate, installed, source_checks)
                if owner is not None:
                    result[coordinate] = self.reviewed(key, owner)
        if self.arch is not None:
            proved = self.arch.prove(arch_candidates)
            proved.update(self.generated_owners(generated, observations, source_checks))
            for coordinate, owner in proved.items():
                platform, _, _, _, (path, sha256, _) = observations[coordinate]
                result[coordinate] = self.reviewed((platform, path, sha256), owner)
        return result

    def reviewed(self, key, owner):
        return {locator: (row, reviews, owner) for locator, (row, reviews) in self.containers[key].items()}

    def producer(self, row):
        return self.packages[row["package_sha256"]]["authority_type"] == "pinned-upstream"

    def trust_owners(self, observations):
        """Current pacman ownership of each observed package-owned trust certificate, by coordinate."""
        candidates = defaultdict(list)
        for coordinate, (platform, _, _, _, (path, sha256, _)) in observations.items():
            row = self.trust_rows.get((path, sha256))
            if row is not None and platform == "linux/amd64" and self.arch is not None:
                candidates[coordinate[:2]].append((coordinate, row, self.packages[row["package_sha256"]]))
        return self.arch.prove(candidates) if candidates else {}

    def generated_owners(self, rows, observations, source_checks):
        """Prove each generated index from the authenticated sources visible at its own layer."""
        if not rows:
            return {}
        visible = {coordinate: self.mime.visible(coordinate[:2]) for coordinate, _ in rows}
        scopes = {coordinate[:2] for coordinate, _ in rows}
        scopes.update((coordinate[0], layer) for coordinate, state in visible.items() if state
                      for layer, _ in state.values())
        views = dict(self.arch.views(scopes))
        result = {}
        for coordinate, row in rows:
            owner = self.generated_owner(coordinate, row, visible[coordinate], views, observations, source_checks)
            if owner is not None:
                result[coordinate] = owner
        return result

    def generated_owner(self, coordinate, row, state, views, observations, source_checks):
        image, layer = coordinate[:2]
        current = views.get((image, layer))
        sources = row["derived_payload"]["sources"]
        # Every visible generator input is pinned, and no package claims the index itself.
        if (state is None or current is None or current[1].get(row["path"])
                or {path for path in state if mime_sources.is_input(path)}
                != {source["path"] for source in sources if mime_sources.is_input(source["path"])}):
            return None
        proved = []
        for source in sources:
            path = source["path"]
            if path not in state:
                return None
            source_layer, entry = state[path]
            observed = observations.get((image, source_layer, entry))
            installed = views.get((image, source_layer))
            if observed is None or installed is None or observed[0] != "linux/amd64" or observed[4][:2] != (
                    path, source["sha256"]):
                return None
            claim = {"path": path, "file_sha256": source["sha256"], "ownership": {"mtree_sha256": source["mtree_sha256"]}}
            package = self.packages[source["package_sha256"]]
            owner = ArchOwnership.owner((image, source_layer), installed, claim, package)
            # The writing layer proves the member; the generator's layer must still install it.
            if owner is None or ArchOwnership.owner((image, layer), current, claim, package, physical=False) is None:
                return None
            proved.append({"path": path, "layer": source_layer, "entry": entry, "sha256": source["sha256"], **owner})
        expected = row["ownership"]["build_inputs"]
        key = tuple(sorted(expected.items()))
        if key not in source_checks:
            source_checks[key] = check_reserved_inputs(self.source, expected)
        if not source_checks[key]:
            return None
        return {"sources": proved, "build_inputs": expected}

    def owner(self, row, coordinate, installed, source_checks):
        package = self.packages[row["package_sha256"]]
        checked = {}
        if package["authority_type"] == "debian-index":
            native = self.native_owner(coordinate, package, allow_inherited=False)
            if native is None:
                return None
            checked["dpkg_status"] = native
        elif "debian_owner" in row["ownership"]:
            # The security-refresh layer can reinstall the compiled binary
            # without rewriting the underlying dpkg database.
            native = self.native_owner(coordinate, row["ownership"]["debian_owner"], allow_inherited=True)
            if native is None:
                return None
            checked["dpkg_status"] = native
        expected = row["ownership"].get("build_inputs")
        if expected is not None:
            key = tuple(sorted(expected.items()))
            if key not in source_checks:
                source_checks[key] = check_reserved_inputs(self.source, expected)
            if not source_checks[key]:
                return None
            checked["build_inputs"] = expected
        if row.get("derived_payload", {}).get("kind") == "mime-index":
            member = row["package_member"]
            if installed.get((*coordinate[:2], member["path"][-1])) != member["sha256"]:
                return None
        return checked

    def native_owner(self, coordinate, package, *, allow_inherited):
        image, layer, _ = coordinate
        name, version, architecture = dpkg_identity(package)
        for index in range(layer, -1, -1) if allow_inherited else (layer,):
            if (image, index) in self.masked_status:
                return None
            if (image, index) in self.status:
                record = self.status[image, index]
                if record is None or record[2].get((name, architecture)) != version:
                    return None
                return {"layer": index, "entry": record[0], "sha256": record[1],
                        "package": dict(zip(("name", "version", "architecture"), (name, version, architecture)))}
        return None
