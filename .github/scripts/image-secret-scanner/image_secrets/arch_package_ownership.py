"""Reconstruct ownership from the ALPM database visible at a physical layer."""

import re
from collections import defaultdict
from pathlib import PurePosixPath
from typing import NamedTuple

from .common import digest_bytes
from .library_vectors import MAX_DATABASE_BYTES, database_contains
from .package_evidence import IDENTIFIER, matches

ROOT = "var/lib/pacman/local"
VERSION = ROOT + "/ALPM_DB_VERSION"
HEADER = re.compile(r"%[A-Z]+%")
METADATA = {"desc", "files", "mtree"}
ANCESTORS = {str(parent) for parent in PurePosixPath(ROOT).parents}


def sections(raw):
    result = {}
    header = None
    try:
        lines = raw.decode("utf-8").split("\n")
    except UnicodeError:
        return None
    for line in lines:
        if HEADER.fullmatch(line):
            if line in result:
                return None
            header = line
            result[header] = []
        elif line:
            if header is None or "\0" in line or "\r" in line:
                return None
            result[header].append(line)
    return result


def identity(raw, directory):
    data = sections(raw)
    if data is None:
        return None
    keys = ("%NAME%", "%VERSION%", "%ARCH%")
    if not all(len(data.get(key, ())) == 1 and matches(IDENTIFIER, data[key][0]) for key in keys):
        return None
    name, version, arch = (data[key][0] for key in keys)
    return (name, version, arch) if directory == f"{name}-{version}" else None


def files_owned(raw, paths):
    data = sections(raw)
    if data is None or data.keys() - {"%FILES%", "%BACKUP%"} or "%BACKUP%" in data and "%FILES%" not in data:
        return None
    observed = set()
    for path in data.get("%FILES%", ()):
        canonical = path.rstrip("/")
        if (not canonical or canonical.startswith("/") or "//" in path
                or any(part in {".", ".."} for part in canonical.split("/"))):
            return None
        if canonical in paths:
            if canonical in observed or path.endswith("/"):
                return None
            observed.add(canonical)
    return observed


class Observation(NamedTuple):
    layer: int
    entry: int
    sha256: str
    value: object

    def receipt(self):
        return {"layer": self.layer, "entry": self.entry, "sha256": self.sha256}


def observe_mask(path, member):
    if path not in ANCESTORS and path != ROOT and not path.startswith(ROOT + "/") and not (
            path.rsplit("/", 1)[-1].startswith(".wh.")):
        return None
    name = PurePosixPath(path)
    if name.name == ".wh..wh..opq":
        return str(name.parent)
    if name.name.startswith(".wh."):
        return str(name.parent / name.name[4:])
    if any("overlay." in key for key in member.pax_headers):
        return path
    if not member.isdir() and (path in ANCESTORS or path == ROOT or name.parent.as_posix() == ROOT and path != VERSION):
        return path
    return None


class ArchOwnership:
    def __init__(self, rows, packages):
        self.targets = set()
        self.verified_members = defaultdict(set)
        self.layers = defaultdict(dict)
        self.masks = defaultdict(set)
        self.invalid = set()
        for row in rows:
            path = row["path"]
            self.targets.add(path)
            origin = row.get("derived_payload")
            # A generated index has no package owner; each of its sources has one.
            claims = ([(source["package_sha256"], source["path"], source["sha256"], source["mtree_sha256"])
                       for source in origin["sources"]] if origin is not None else
                      [(row["package_sha256"], path, row["file_sha256"], row["ownership"]["mtree_sha256"])])
            for artifact, member, sha256, mtree in claims:
                package = packages[artifact]
                self.targets.add(member)
                # Member rows of one container share its single installed checksum record.
                self.verified_members[f'{package["name"]}-{package["version"]}'].add((member, sha256, mtree))

    def observe(self, layer, member, path, coordinate, budget):
        scope = coordinate[:2]
        entries = self.layers[scope]
        mask = observe_mask(path, member)
        if mask is not None:
            if mask == "." or mask in ANCESTORS or mask == ROOT or mask.startswith(ROOT + "/"):
                if PurePosixPath(path).name.startswith(".wh.") and member.isfile() and member.size == 0:
                    self.masks[scope].add(mask)
                else:
                    self.invalid.add(scope)
            return
        parent, _, name = path.rpartition("/")
        relative = parent.removeprefix(ROOT + "/")
        if path != VERSION and (name not in METADATA or relative == parent or not relative or "/" in relative):
            return
        if name == "mtree" and relative not in self.verified_members:
            return
        if path in entries or not member.isfile() or member.issparse() or member.size > MAX_DATABASE_BYTES:
            entries[path] = None
            return
        budget.reserve(member.size)
        with layer.extractfile(member) as stream:
            data = stream.read(member.size + 1)
        if len(data) != member.size:
            entries[path] = None
            return
        checksum = digest_bytes(data)
        if path == VERSION:
            value = data == b"9\n"
        elif name == "desc":
            value = identity(data, relative)
        elif name == "files":
            value = files_owned(data, self.targets)
        else:
            value = {(target, sha) for target, sha, mtree in self.verified_members[relative]
                     if checksum == mtree and database_contains(data, target, sha, debian=False)}
        entries[path] = Observation(coordinate[1], coordinate[2], checksum, value)

    def snapshots(self):
        image = None
        entries = {}
        blocked = False
        for scope, records in sorted(self.layers.items()):
            if scope[0] != image:
                entries = {}
                image = scope[0]
                blocked = False
            blocked |= scope in self.invalid
            if blocked:
                continue
            for path in self.masks.get(scope, ()):
                entries = {name: row for name, row in entries.items()
                           if path != "." and name != path and not name.startswith(path + "/")}
            entries.update(records)
            yield scope, entries

    def views(self, scopes):
        """Index each requested layer's complete installed-package database."""
        for scope, entries in self.snapshots():
            if scope not in scopes:
                continue
            version = entries.get(VERSION)
            if version is None or not version.value:
                continue
            owners = defaultdict(list)
            descriptions = {}
            complete = True
            for path, record in entries.items():
                if not path.endswith(("/desc", "/files")):
                    continue
                if record is None or record.value is None:
                    complete = False
                    break
                if path.endswith("/desc"):
                    descriptions[path.removesuffix("/desc")] = record.value
                else:
                    for file in record.value:
                        owners[file].append(path.removesuffix("/files"))
            files = {name.removesuffix("/files") for name in entries if name.endswith("/files")}
            if not complete or files != descriptions.keys() or len({d[0] for d in descriptions.values()}) != len(files):
                continue
            # Later layers keep updating the snapshot; a view must stay at its own layer.
            yield scope, (dict(entries), owners, descriptions)

    @staticmethod
    def owner(scope, view, row, package, *, physical=True):
        """Unique owner of the exact file; a physical claim also needs its records in that layer."""
        entries, owners, descriptions = view
        key = (package["name"], package["version"], package["architecture"])
        directory = f"{ROOT}/{key[0]}-{key[1]}"
        if owners.get(row["path"]) != [directory] or descriptions.get(directory) != key:
            return None
        database = {name: entries.get(f"{directory}/{name}") for name in sorted(METADATA)}
        mtree = database["mtree"]
        if (mtree is None or mtree.sha256 != row["ownership"]["mtree_sha256"]
                or (row["path"], row["file_sha256"]) not in mtree.value
                or any(record is None or physical and record.layer != scope[1] for record in database.values())):
            return None
        return {"pacman": {name: record.receipt() for name, record in database.items()}}

    def prove(self, rows_by_layer):
        eligible = {}
        for scope, view in self.views(rows_by_layer):
            for coordinate, row, package in rows_by_layer[scope]:
                owner = self.owner(scope, view, row, package)
                if owner is not None:
                    eligible[coordinate] = owner
        return eligible
