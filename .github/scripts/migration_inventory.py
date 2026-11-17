#!/usr/bin/env python3
"""Discover the AM collection floor from two immutable source revisions.

This is an inventory, not an execution result or a Rust type checker. Unknown
registry syntax is an error: dropping a newly added collection is unsafe.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess


ROOT = "api/crates/cma/src/"
REGISTRIES = ("store::isolation", "store::isolation::replicas")
# Separate operation roles and retained adapters are outside the runtime role.
EXTRA_SYMBOLS = (
    "store::isolation::MIGRATION_COLLECTION",
    "store::provenance_progress::model::COLLECTION",
    "store::gateway_records::GATEWAY_CHANNEL_RECORDS",
    "store::gateway_interactions::COLLECTION",
)


def digest(value):
    return hashlib.sha256(value).hexdigest()


class Source:
    def __init__(self, repo, revision):
        if not re.fullmatch(r"[a-f0-9]{40}", revision):
            raise ValueError("full immutable source SHA required")
        self.repo, self.revision, self.cache = Path(repo), revision, {}
        self.tree = self.git("rev-parse", revision + "^{tree}").strip()

    def git(self, *args):
        return subprocess.check_output(
            ["git", "-C", str(self.repo), *args], text=True, stderr=subprocess.PIPE)

    def read(self, path):
        if path not in self.cache:
            self.cache[path] = self.git("show", self.revision + ":" + path)
        return self.cache[path]

    def module(self, name):
        base = ROOT + name.replace("::", "/")
        for path in (base + ".rs", base + "/mod.rs"):
            try:
                return path, self.read(path)
            except subprocess.CalledProcessError:
                continue
        raise ValueError("missing source module: " + name)

    def constant(self, symbol, seen=()):
        if symbol in seen:
            raise ValueError("cyclic constant: " + symbol)
        module, name = symbol.rsplit("::", 1)
        path, source = self.module(module)
        pattern = rf'\bconst\s+{re.escape(name)}\s*:\s*&str\s*=\s*"([a-z0-9_]+)"\s*;'
        match = re.search(pattern, source)
        if match:
            return match[1], {"path": path, "sha256": digest(source.encode()),
                              "symbol": symbol}
        # Resolve explicit re-exports such as document_artifacts::{mongo,quota}.
        exports = re.finditer(r'\buse\s+([\w:]+)::(?:\{([^}]+)\}|(\w+))\s*;', source)
        for export in exports:
            names = [part.strip() for part in (export[2] or export[3]).split(",")]
            if name in names:
                target = self.resolve(module, export[1] + "::" + name)
                return self.constant(target, (*seen, symbol))
        raise ValueError("unresolved collection constant: " + symbol)

    @staticmethod
    def resolve(module, expression):
        parts = expression.split("::")
        prefix = module.split("::")
        if parts[0] == "crate":
            prefix, parts = [], parts[1:]
        while parts and parts[0] in ("super", "self"):
            if parts.pop(0) == "super":
                if not prefix:
                    raise ValueError("collection path escapes crate")
                prefix.pop()
        return "::".join(prefix + parts)

    def registry(self, module):
        path, source = self.module(module)
        match = re.search(r'\bconst\s+COLLECTIONS:\s*&\[&str\]\s*=\s*&\[(.*?)\];',
                          source, re.S)
        if not match:
            raise ValueError("unrecognized collection registry: " + path)
        body = re.sub(r"//[^\n]*", "", match[1])
        rows = []
        for expression in filter(None, (part.strip() for part in body.split(","))):
            if re.fullmatch(r'"[a-z0-9_]+"', expression):
                name = json.loads(expression)
                definition = {"path": path, "sha256": digest(source.encode()),
                              "symbol": module + "::COLLECTIONS"}
            elif re.fullmatch(r"[A-Za-z_][\w:]*", expression):
                name, definition = self.constant(self.resolve(module, expression))
            else:
                raise ValueError("unrecognized collection expression: " + expression)
            rows.append((name, definition))
        if len({name for name, _ in rows}) != len(rows):
            raise ValueError("duplicate collection in " + path)
        return rows

    def collections(self):
        result = {}
        for module in REGISTRIES:
            for name, definition in self.registry(module):
                if name in result:
                    raise ValueError("duplicate runtime/replica collection: " + name)
                result[name] = {"role": "runtime", "definition": definition}
        for symbol in EXTRA_SYMBOLS:
            name, definition = self.constant(symbol)
            result[name] = {"role": "separate-or-retained", "definition": definition}
        path = ROOT + "store/gateway_records.rs"
        source = self.read(path)
        match = re.search(r'Self::open_namespace\(client, database, "([a-z0-9_]+)"\)', source)
        if not match:
            raise ValueError("retained gateway connection namespace needs review")
        result[match[1]] = {"role": "separate-or-retained", "definition": {
            "path": path, "sha256": digest(source.encode()),
            "symbol": "store::gateway_records::open_connections"}}
        return result


def compare(old, new):
    before, after = old.collections(), new.collections()
    return {
        "schema": "cma-reader-writer-source-inventory-v1",
        "old": {"source": old.revision, "tree": old.tree},
        "new": {"source": new.revision, "tree": new.tree},
        "scope": "AM runtime catalog, replica catalog, migration/provenance roles and retained gateway adapters",
        "shape_coverage": "REQUIRES_REVIEWED_FIXTURES; collection discovery is not nested-shape coverage",
        "collections": [{"name": name, "old": before.get(name), "new": after.get(name),
                         "old_to_new": "NOT_RUN", "new_to_old": "NOT_RUN"}
                        for name in sorted(before.keys() | after.keys())],
    }


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, required=True)
    parser.add_argument("--old", required=True)
    parser.add_argument("--new", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args(argv)
    inventory = compare(Source(args.repo, args.old), Source(args.repo, args.new))
    # Never overwrite a campaign's source identity when integration advances.
    with args.output.open("x") as stream:
        json.dump(inventory, stream, indent=2)
        stream.write("\n")
    print(json.dumps({"collections": len(inventory["collections"]),
                      "sha256": digest(args.output.read_bytes()), "execution": "NOT_RUN"}))


if __name__ == "__main__":
    main()
