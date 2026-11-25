"""Product ownership policy over Cargo's workspace dependency graph."""

from pathlib import Path, PurePosixPath
import re

# cma-journey-probe is a standalone public-journey runner. It is in no image, and
# its tests reuse Agent Manager wire fixtures, so the AM track owns it.
AM = frozenset({"cma", "cma-image-manager", "cma-allocator-qualification"})
AM_OPTIONAL = frozenset({"cma-journey-probe"})
EG = frozenset({"cmaeg"})
SHARED = frozenset({"layer1-public", "layer2-core", "layer3-adapter",
                    "layer4-runtime", "cma-service-auth", "cmaeg-core"})
TRACKS = frozenset({"am", "eg", "web"})
BUILD_OWNERS = {
    "api/crates/cma/Dockerfile": {"am"},
    "api/crates/cma-image-manager/Dockerfile": {"am"},
    "api/crates/layer4-runtime/Dockerfile": {"am"},
    "api/crates/cmaeg/Dockerfile": {"eg"},
}


def closure(seeds, edges):
    result = set(seeds)
    pending = list(seeds)
    while pending:
        for name in edges[pending.pop()] - result:
            result.add(name)
            pending.append(name)
    return result


class Workspace:
    def __init__(self, metadata, root):
        root = Path(root).resolve()
        if Path(metadata["workspace_root"]).resolve() != root / "api":
            raise ValueError("unexpected workspace root")
        packages = metadata["packages"]
        if {p["id"] for p in packages} != set(metadata["workspace_members"]):
            raise ValueError("incomplete workspace metadata")
        self.names = {p["name"] for p in packages}
        if len(self.names) != len(packages) or not (AM | EG | SHARED) <= self.names:
            raise ValueError("missing or ambiguous package ownership")
        if any(not re.fullmatch(r"[A-Za-z0-9_-]+", name) for name in self.names):
            raise ValueError("invalid package name")
        self.directories = {}
        self.build_scripts = {}
        for p in packages:
            manifest = Path(p["manifest_path"]).resolve()
            if manifest.name != "Cargo.toml":
                raise ValueError("invalid package manifest")
            relative = manifest.parent.relative_to(root).as_posix()
            if not relative.startswith("api/") or relative in self.directories:
                raise ValueError("ambiguous package directory")
            self.directories[relative] = p["name"]
            self.build_scripts[relative] = {
                Path(target["src_path"]).resolve().relative_to(root).as_posix()
                for target in p.get("targets", []) if "custom-build" in target["kind"]
            }
        by_path = {str((root / path).resolve()): name
                   for path, name in self.directories.items()}
        self.dependencies = {name: set() for name in self.names}
        self.reverse = {name: set() for name in self.names}
        self.production = {name: set() for name in self.names}
        for p in packages:
            # no-deps metadata retains path and dependency kind, including renamed,
            # optional, target-specific, build and dev workspace dependencies.
            for dep in p["dependencies"]:
                if dep.get("path") is None:
                    continue
                target = by_path.get(str(Path(dep["path"]).resolve()))
                if target is None:
                    raise ValueError("unclassified local dependency")
                self.dependencies[p["name"]].add(target)
                if dep.get("kind") != "dev":
                    self.production[p["name"]].add(target)
                self.reverse[target].add(p["name"])
        am_roots = AM | SHARED | (AM_OPTIONAL & self.names)
        self.owned = {
            "am": closure(am_roots, self.dependencies) - EG,
            "eg": closure(EG, self.dependencies) | {"cmaeg-core", "cma-service-auth", "cma-allocator-qualification"},
        }
        if self.owned["am"] | self.owned["eg"] != self.names:
            raise ValueError("workspace package has no product owner")

    def affected(self, path):
        matches = [name for directory, name in self.directories.items()
                   if path == directory or path.startswith(directory + "/")]
        if len(matches) != 1:
            raise ValueError("unclassified workspace path")
        return closure(matches, self.reverse)


def select(paths, workspace=None, force_all=False):
    selected, affected, reasons = set(), set(), []
    if force_all:
        selected.update(TRACKS)
        reasons.append("all-tracks-event-or-uncertain-diff")
    for path in paths:
        parts = PurePosixPath(path).parts
        if not path or path.startswith("/") or ".." in parts:
            raise ValueError("invalid changed path")
        if path.startswith(".github/workflows/"):
            reasons.append("workflow-repo-only")
        elif path in BUILD_OWNERS:
            selected.update(BUILD_OWNERS[path])
            reasons.append("component-build-recipe")
        elif path.startswith("api/crates/cmaeg/assets/docs/"):
            selected.update({"eg", "web"})
            reasons.append("gateway-and-web-documentation-assets")
        elif path.startswith("web/"):
            selected.add("web")
            reasons.append("web-source")
        elif path in {"api/Cargo.toml", "api/Cargo.lock", "api/rust-toolchain.toml"} or path.startswith("api/.cargo/"):
            selected.update({"am", "eg"})
            affected.update(SHARED)
            reasons.append("workspace-configuration")
        elif path.startswith("api/crates/") and workspace is not None:
            try:
                changed = workspace.affected(path)
            except ValueError:
                selected.update(TRACKS)
                reasons.append("unclassified-workspace-path")
                continue
            affected.update(changed)
            if changed & (AM | SHARED):
                selected.add("am")
            if changed & (EG | SHARED):
                selected.add("eg")
            # A new package inherits its product ownership from the graph.
            for track, owned in workspace.owned.items():
                if changed & (owned - AM - EG - SHARED):
                    selected.add(track)
            reasons.append("workspace-reverse-dependencies")
        else:
            selected.update(TRACKS)
            reasons.append("shared-or-unclassified-path")
    result = {track: track in selected for track in TRACKS}
    result.update(repo=True, reasons=sorted(set(reasons)), affected=sorted(affected))
    for track in ("am", "eg"):
        if not result[track]:
            packages = []
        elif workspace is None:
            # Unknown graph must broaden test coverage as well as job selection.
            packages = ["*"]
        else:
            extra = SHARED if force_all or selected == TRACKS else affected & SHARED
            packages = sorted(workspace.owned[track] | extra)
        result[track + "_packages"] = packages
    return result
