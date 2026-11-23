"""Bind a generated Shared MIME-info index to every authenticated input of its generator."""

import json
from collections import defaultdict
from pathlib import PurePosixPath

from .common import digest_bytes
from .gate import valid_digest
from .package_evidence import NESTED_LOCATOR, fields, matches, normalized, require

PACKAGES = "usr/share/mime/packages"
SOURCE = PACKAGES + "/freedesktop.org.xml"
# The pacman hook supplies the generator invocation and its settings.
GENERATOR = "usr/bin/update-mime-database"
HOOK = "usr/share/libalpm/hooks/30-update-mime-database.hook"
FIXED = frozenset({GENERATOR, HOOK})
ANCESTORS = frozenset(str(parent) for path in (PACKAGES, *FIXED) for parent in PurePosixPath(path).parents)
SOURCE_FIELDS = frozenset({"path", "sha256", "package_sha256", "package_member", "mtree_sha256"})
MEMBER_FIELDS = frozenset({"locator", "path", "type", "sha256"})
MAX_SOURCES = 64


def is_input(path):
    location = PurePosixPath(path)
    return str(location.parent) == PACKAGES and location.suffix == ".xml" and not location.name.startswith(".")


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode()


def identity_sha256(package):
    return digest_bytes(canonical({key: package[key] for key in ("name", "version", "architecture",
                                                                   "artifact_sha256")}))


def inputs_sha256(sources):
    return digest_bytes(canonical([[source["path"], source["sha256"]] for source in sources]))


def validate(row, package, packages):
    """Each source is a signed member with its own native owner; the producer names exactly these."""
    proof = row["derived_payload"]
    sources = proof["sources"]
    require(isinstance(sources, list) and 0 < len(sources) <= MAX_SOURCES)
    paths = []
    for source in sources:
        fields(source, SOURCE_FIELDS)
        path = normalized(source["path"])
        member = source["package_member"]
        fields(member, MEMBER_FIELDS)
        artifact = source["package_sha256"]
        require((is_input(path) or path in FIXED) and valid_digest(source["sha256"])
                and valid_digest(source["mtree_sha256"]) and valid_digest(artifact) and artifact in packages
                and packages[artifact]["authority_type"] == "arch-signature"
                and packages[artifact]["architecture"] in {"x86_64", "any"}
                and (path not in FIXED or artifact == row["package_sha256"])
                and matches(NESTED_LOCATOR, member["locator"]) and member["type"] == "regular"
                and member["sha256"] == source["sha256"] and isinstance(member["path"], list)
                and len(member["path"]) == member["locator"].count("/") + 1)
        for part in member["path"]:
            normalized(part)
        require(member["path"][-1] == path)
        paths.append(path)
    require(paths == sorted(set(paths)) and FIXED <= set(paths) and SOURCE in paths)
    [signed] = [source for source in sources if source["path"] == SOURCE]
    require(signed["package_sha256"] == row["package_sha256"] and signed["package_member"] == row["package_member"])
    producer = proof["producer"]
    require(producer["identity_sha256"] == identity_sha256(package)
            and producer["inputs_sha256"] == inputs_sha256(sources))


class VisibleSources:
    """Replay the generator's namespace from physical entries; anything unusual refuses."""

    def __init__(self):
        self.events = defaultdict(list)
        self.invalid = set()

    def observe(self, member, path, coordinate):
        location = PurePosixPath(path)
        name, parent = location.name, str(location.parent)
        scope, entry = coordinate[:2], coordinate[2]
        if name.startswith(".wh."):
            if name == ".wh..wh..opq":
                if parent == PACKAGES:
                    self.events[scope].append(("clear", parent, entry))
                elif parent in ANCESTORS:
                    self.invalid.add(scope)
                return
            hidden = str(location.parent / name[4:])
            if str(PurePosixPath(hidden).parent) == PACKAGES or hidden in FIXED:
                self.events[scope].append(("delete", hidden, entry))
            elif hidden == PACKAGES or hidden in ANCESTORS:
                self.invalid.add(scope)
            return
        regular = member.isfile() and not member.issparse()
        if parent == PACKAGES or path in FIXED:
            if regular and (path in FIXED or is_input(path)):
                self.events[scope].append(("add", path, entry))
            else:
                self.invalid.add(scope)
        elif path.startswith(PACKAGES + "/"):
            self.invalid.add(scope)
        elif path == PACKAGES or path in ANCESTORS:
            opaque = any("overlay." in key for key in member.pax_headers)
            if not member.isdir() or opaque and path != PACKAGES:
                self.invalid.add(scope)
            elif opaque:
                self.events[scope].append(("clear", path, entry))

    def visible(self, scope):
        """Return path -> (layer, entry) for the sources visible after this physical layer."""
        image, top = scope
        state = {}
        for layer in range(top + 1):
            here = (image, layer)
            if here in self.invalid:
                return None
            events = self.events.get(here, ())
            # A layer's whiteouts apply to lower layers before its own entries.
            for action, path, _ in events:
                if action == "clear":
                    state = {name: value for name, value in state.items() if name in FIXED}
                elif action == "delete":
                    state.pop(path, None)
            added = [(path, entry) for action, path, entry in events if action == "add"]
            if len({path for path, _ in added}) != len(added):
                return None
            for path, entry in added:
                state[path] = (layer, entry)
        return state
