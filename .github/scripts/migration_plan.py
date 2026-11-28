#!/usr/bin/env python3
"""Declare a release matrix or a legacy two-image pairwise probe.

The inventory discovers every collection name in both source snapshots. This
step expands each name into retained, disabled, and enabled writer fixtures so
the artifact runner cannot accidentally omit a policy direction. It does not
claim that any cell passed. Release declarations require all four artifact
roles and reviewed expectations before execution; neither mode executes images.
"""
import argparse
import hashlib
import json
import os
import re
import stat
import sys
from pathlib import Path


SOURCE = re.compile(r"^[a-f0-9]{40}$")
IMAGE = re.compile(r"^(?:[a-z0-9](?:[a-z0-9.-]*[a-z0-9])?(?::[0-9]+)?/)+"
                   r"[a-z0-9][a-z0-9._/-]*@sha256:[a-f0-9]{64}$")
ID = re.compile(r"^[a-z][a-z0-9-]{0,95}$")
ROLES = ("historical", "current", "candidate", "recovery")
POLICIES = ("retained", "disabled", "enabled")
MAX_CELLS = 4096
MAX_BYTES = 4 * 1024 * 1024


def digest(data):
    return hashlib.sha256(data).hexdigest()


def identity(source, image):
    if not isinstance(source, str) or not SOURCE.fullmatch(source):
        raise ValueError("full immutable source SHA required")
    if not isinstance(image, str) or not IMAGE.fullmatch(image):
        raise ValueError("full immutable OCI image digest required")
    return {"source": source, "image": image}


def slug(name):
    if not isinstance(name, str):
        raise ValueError("collection name must be a string")
    value = re.sub(r"[^a-z0-9]+", "-", name.lower()).strip("-")
    if not value or len(value) > 80:
        raise ValueError("collection name cannot become a bounded fixture id")
    return value


def fields(value, names):
    if not isinstance(value, dict) or set(value) != set(names):
        raise ValueError("input object has missing or unknown fields")


def checked_id(value):
    if not isinstance(value, str) or not ID.fullmatch(value):
        raise ValueError("matrix id must be bounded and syntactically valid")
    return value


def selections(artifacts):
    fields(artifacts, ROLES)
    result = {}
    for role in ROLES:
        fields(artifacts[role], ("source", "image"))
        result[role] = identity(**artifacts[role])
    component = result["candidate"]["image"].split("@")[0]
    if any(row["image"].split("@")[0] != component for row in result.values()):
        raise ValueError("all artifacts must name the same component")
    return result


def fixtures_for(inventory, old, new):
    rows = inventory.get("collections") if isinstance(inventory, dict) else None
    # A collection adds every reader/policy combination to the matrix.
    if not isinstance(rows, list) or not 0 < len(rows) * len(POLICIES) * len(ROLES) <= MAX_CELLS:
        raise ValueError("inventory must fit the matrix cell limit")
    if any(not isinstance(row, dict) or not isinstance(row.get("name"), str) for row in rows):
        raise ValueError("inventory collections require names")
    names = [row["name"] for row in rows]
    if len(names) != len(set(names)):
        raise ValueError("inventory collection names must be non-empty and unique")
    slugs = [slug(name) for name in names]
    if len(slugs) != len(set(slugs)):
        raise ValueError("collection fixture ids would collide")
    fixtures = []
    for name, key in zip(names, slugs):
        for policy in POLICIES:
            writer = old if policy == "retained" else new
            fixture_id = checked_id(f"collection-{key}-{policy}")
            material = json.dumps({"collection": name, "policy": policy,
                                   "writer": writer}, sort_keys=True).encode()
            fixtures.append({"id": fixture_id, "sha256": digest(material),
                             "writer": writer, "policy": policy})
    return fixtures


def reviewed_cells(expectations, pairs):
    if not isinstance(expectations, list) or not 0 < len(expectations) <= MAX_CELLS:
        raise ValueError("complete reviewed expectations required")
    required = set(pairs)
    selected, ids = {}, set()
    for cell in expectations:
        fields(cell, ("id", "reader", "fixture", "expectation"))
        cell_id = checked_id(cell["id"])
        pair = (checked_id(cell["reader"]), checked_id(cell["fixture"]))
        if pair not in required or pair in selected or cell_id in ids:
            raise ValueError("unknown or duplicate matrix cell")
        if cell["expectation"] not in ("accept", "refuse"):
            raise ValueError("cell expectation must be accept or refuse")
        if pair[0] in ("candidate", "recovery") and cell["expectation"] != "accept":
            raise ValueError("candidate and recovery must accept every fixture")
        selected[pair] = dict(cell)
        ids.add(cell_id)
    if selected.keys() != required:
        raise ValueError("reviewed expectations omit matrix cells")
    return [selected[pair] for pair in pairs]


def construct(inventory, artifacts, expectations=None):
    readers = [{"id": role, "role": role, "artifact": artifacts[role]} for role in ROLES]
    fixtures = fixtures_for(inventory, artifacts["historical"], artifacts["candidate"])
    pairs = [(reader["id"], fixture["id"]) for reader in readers for fixture in fixtures]
    if expectations is None:
        # Only the pairwise probe supplies this compatibility default. Changed
        # expectations require a new reviewed plan/hash and a fresh campaign.
        expectations = [{"id": f"{reader}-{fixture}", "reader": reader,
                         "fixture": fixture, "expectation": "accept"}
                        for reader, fixture in pairs]
    cells = reviewed_cells(expectations, pairs)
    return {"schema": "cma-migration-matrix-plan-v1", "candidate": artifacts["candidate"],
            "readers": readers, "fixtures": fixtures, "cells": cells}


def build(inventory, old_source, old_image, new_source, new_image):
    """Build a pairwise probe, retaining the legacy current/recovery aliases."""
    old = identity(old_source, old_image)
    new = identity(new_source, new_image)
    return construct(inventory, selections({"historical": old, "current": new,
                                           "candidate": new, "recovery": old}))


def build_release(inventory, artifacts, expectations):
    """Declare explicit release readers and expectations; execution is NOT_RUN."""
    artifacts = selections(artifacts)
    if not isinstance(inventory, dict) or inventory.get("schema") != "cma-reader-writer-source-inventory-v1":
        raise ValueError("release requires a source inventory")
    for version, role in (("old", "historical"), ("new", "candidate")):
        source = inventory.get(version)
        fields(source, ("source", "tree"))
        if (source["source"] != artifacts[role]["source"]
                or not isinstance(source["tree"], str) or not SOURCE.fullmatch(source["tree"])):
            raise ValueError("source inventory identity does not match selected artifacts")
    old = artifacts["historical"]
    if any(row["source"] == old["source"] and row != old for row in artifacts.values()):
        raise ValueError("readers of the historical source must share its exact artifact")
    if expectations is None:
        raise ValueError("complete reviewed expectations required")
    return construct(inventory, artifacts, expectations)


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("duplicate JSON key")
        result[key] = value
    return result


def reject_constant(_value):
    raise ValueError("non-JSON numeric constant")


def read_input(path):
    # Nonblocking open plus fstat rejects FIFOs without waiting for a writer.
    with os.fdopen(os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK), "rb") as stream:
        info = os.fstat(stream.fileno())
        if not stat.S_ISREG(info.st_mode) or not 0 < info.st_size <= MAX_BYTES:
            raise ValueError("input must be a bounded regular file")
        data = stream.read(MAX_BYTES + 1)
    if not 0 < len(data) <= MAX_BYTES:
        raise ValueError("input must be a bounded regular file")
    return json.loads(data, object_pairs_hook=unique_object, parse_constant=reject_constant)


class SafeParser(argparse.ArgumentParser):
    def error(self, _message):
        # argparse's default error embeds untrusted arguments and paths.
        super().error("invalid migration plan arguments; use --help")


def main(argv=None):
    parser = SafeParser(prog="migration_plan.py", description=__doc__, allow_abbrev=False)
    parser.add_argument("--inventory", type=Path, required=True)
    for flag in ("old-source", "old-image", "new-source", "new-image"):
        parser.add_argument("--" + flag, help="legacy pairwise probe input")
    parser.add_argument("--artifacts", type=Path, help="release declaration: four explicit artifact roles")
    parser.add_argument("--expectations", type=Path, help="release declaration: reviewed complete cell list")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args(argv)
    probe = (args.old_source, args.old_image, args.new_source, args.new_image)
    release = (args.artifacts, args.expectations)
    if any(value is not None for value in release):
        if not all(value is not None for value in release) or any(value is not None for value in probe):
            parser.error("release requires artifacts and expectations without probe inputs")
        mode = "release-declaration"
        plan = build_release(read_input(args.inventory), read_input(args.artifacts),
                             read_input(args.expectations))
    else:
        if not all(value is not None for value in probe):
            parser.error("pairwise probe requires both immutable images and sources")
        mode = "pairwise-probe"
        plan = build(read_input(args.inventory), *probe)
    data = (json.dumps(plan, indent=2) + "\n").encode("utf-8")
    if len(data) > MAX_BYTES:
        raise ValueError("plan exceeds the matrix file size limit")
    with args.output.open("xb") as stream:
        stream.write(data)
    print(json.dumps({"mode": mode, "execution": "NOT_RUN", "fixtures": len(plan["fixtures"]),
                      "cells": len(plan["cells"]),
                      "sha256": digest(data)}))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, RecursionError):
        print("Migration plan input or output is invalid; raw diagnostics withheld", file=sys.stderr)
        sys.exit(1)
