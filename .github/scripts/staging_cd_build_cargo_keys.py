"""Lookup and compatibility boundaries for protected Cargo mount archives."""
import hashlib
import json
import re
import tomllib
from dataclasses import dataclass
from datetime import datetime, timezone

from staging_cd_versions import DEPLOYABLES

CARGO_NAMESPACE = "cma-cd-cargo-v4-"
GRAPH_TABLES = {"dependencies", "dev-dependencies", "build-dependencies", "features", "target",
                "patch", "replace"}
PACKAGE_GRAPH_FIELDS = {"name", "version", "edition", "resolver", "rust-version", "links"}
RUST_STAGES = ("chef", "planner", "dependencies", "build")


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True).encode()).hexdigest()


def compiler_recipe(path):
    # Native packages, build flags and mount layout can change the meaning of
    # compiler state. Runtime packaging edits outside these stages cannot.
    recipe = path.read_text()
    stages = re.split(r"(?im)^(?=FROM\s)", recipe)
    compiler = []
    for stage in stages[1:]:
        match = re.fullmatch(r"FROM (\S+) AS (\w+)", stage.splitlines()[0], re.I)
        if match and match[2].lower() in RUST_STAGES:
            base, name = match[1], match[2].lower()
            if name in ("chef", "build") and not re.fullmatch(r"rust:[\w.@:-]+", base):
                return recipe
            if name in ("planner", "dependencies") and base != "chef":
                return recipe
            if name == "build":
                # The trim and runtime self-tests follow this copy. They do not
                # compile Rust, and the archive validator checks the trim result.
                output = re.search(r"(?m)^\s*&& cp /build/target/release/(cma|cmaeg) /out/\1(?: \\)?$", stage)
                if not output:
                    return recipe
                stage = stage[:output.end()]
            compiler.append((name, stage))
    if tuple(name for name, _ in compiler) != RUST_STAGES:
        # A historical or unfamiliar stage layout gets a recipe-specific family.
        return recipe
    # External stage inputs and global variable expansion need their complete recipe.
    texts = [stage for _, stage in compiler]
    references = re.findall(r"(?:--|,)from=([^,\s]+)", "\n".join(texts), re.I)
    if any(name not in RUST_STAGES for name in references):
        return recipe
    # Global ARG defaults can feed a Rust stage through a later ARG or ENV.
    return [stages[0], *texts]


@dataclass(frozen=True)
class CargoKey:
    family: str
    graph: str
    source: str = ""

    @property
    def prefix(self):
        generation = digest({"graph": self.graph, "source": self.source}) if self.source else self.graph
        return self.family + generation + "-"

    def archive_key(self, env):
        return self.prefix + env["GITHUB_RUN_ID"] + "-" + env["GITHUB_RUN_ATTEMPT"]


def identity(root, component, *, day=None):
    recipe = json.loads((root / ".github/scripts/deployables.json").read_text())[DEPLOYABLES[component]]
    api = root / "api"
    workspace = tomllib.loads((api / "Cargo.toml").read_text())
    configs = {p.relative_to(api).as_posix(): hashlib.sha256(p.read_bytes()).hexdigest()
               for p in sorted((api / ".cargo").rglob("*")) if p.is_file()}
    # The Rust base tag and native Alpine packages can change independently of
    # Git, so the exact key still turns over each UTC day. The day is part of the
    # graph, not the family: across the boundary a bundle may seed only rustc's
    # incremental state, as across a dependency change. Rustc validates that
    # state against its compiler and every upstream crate, and the bundle never
    # carries dependency objects, build-script output or the registry across it.
    # In the family, the first build of each day recompiled the whole workspace
    # from nothing (AM 32 minutes on 21 Nov 2026 instead of 14).
    refresh = day if day is not None else datetime.now(timezone.utc).date()
    family = digest({"platform": "linux/amd64",
                     "target": "x86_64-unknown-linux-musl", "profile": "release",
                     "release_profile": workspace.get("profile", {}).get("release", {}),
                     "config": configs, "toolchain": tomllib.loads((api / "rust-toolchain.toml").read_text()),
                     "compiler": compiler_recipe(root / recipe["dockerfile"])})
    manifests = {}
    for path in [api / "Cargo.toml", *sorted((api / "crates").glob("*/Cargo.toml"))]:
        manifest = workspace if path == api / "Cargo.toml" else tomllib.loads(path.read_text())
        tables = {k: v for k, v in manifest.items() if k in GRAPH_TABLES}
        tables["package"] = {k: v for k, v in manifest.get("package", {}).items() if k in PACKAGE_GRAPH_FIELDS}
        if "workspace" in manifest:
            settings = manifest["workspace"]
            tables["workspace"] = {k: settings[k] for k in ("members", "default-members", "exclude",
                                   "resolver", "dependencies") if k in settings}
            tables["workspace"]["package"] = {k: v for k, v in settings.get("package", {}).items()
                                              if k in PACKAGE_GRAPH_FIELDS}
        manifests[path.relative_to(api).as_posix()] = tables
    graph = digest({"refresh_day": refresh.isoformat(),
                    "lock": hashlib.sha256((api / "Cargo.lock").read_bytes()).hexdigest(), "manifests": manifests})
    return CargoKey(f"{CARGO_NAMESPACE}{component}-{family}-", graph)
