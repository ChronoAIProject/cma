#!/usr/bin/env python3
"""Create the executable reader/writer plan from an immutable inventory.

The inventory discovers every collection name in both source snapshots. This
step expands each name into retained, disabled, and enabled writer fixtures so
the artifact runner cannot accidentally omit a policy direction. It does not
claim that any cell passed; only the kind runner may write evidence.
"""
import argparse
import hashlib
import json
import re
from pathlib import Path


SOURCE = re.compile(r"^[a-f0-9]{40}$")
IMAGE = re.compile(r"^(?:[a-z0-9](?:[a-z0-9.-]*[a-z0-9])?(?::[0-9]+)?/)+"
                   r"[a-z0-9][a-z0-9._/-]*@sha256:[a-f0-9]{64}$")


def digest(data):
    return hashlib.sha256(data).hexdigest()


def identity(source, image):
    if not SOURCE.fullmatch(source):
        raise ValueError("full immutable source SHA required")
    if not IMAGE.fullmatch(image):
        raise ValueError("full immutable OCI image digest required")
    return {"source": source, "image": image}


def slug(name):
    value = re.sub(r"[^a-z0-9]+", "-", name.lower()).strip("-")
    if not value or len(value) > 80:
        raise ValueError("collection name cannot become a bounded fixture id")
    return value


def build(inventory, old_source, old_image, new_source, new_image):
    old = identity(old_source, old_image)
    new = identity(new_source, new_image)
    names = [row["name"] for row in inventory.get("collections", [])]
    if not names or len(names) != len(set(names)):
        raise ValueError("inventory collection names must be non-empty and unique")
    slugs = [slug(name) for name in names]
    if len(slugs) != len(set(slugs)):
        raise ValueError("collection fixture ids would collide")
    readers = [
        {"id": "historical", "role": "historical", "artifact": old},
        {"id": "current", "role": "current", "artifact": new},
        {"id": "candidate", "role": "candidate", "artifact": new},
        {"id": "recovery", "role": "recovery", "artifact": old},
    ]
    fixtures = []
    for name, key in zip(names, slugs):
        for policy, writer in (("retained", old), ("disabled", new), ("enabled", new)):
            fixture_id = f"collection-{key}-{policy}"
            material = json.dumps({"collection": name, "policy": policy,
                                   "writer": writer}, sort_keys=True).encode()
            fixtures.append({"id": fixture_id, "sha256": digest(material),
                             "writer": writer, "policy": policy,
                             "collection": name})
    # The validator deliberately accepts only the public five-key fixture
    # contract. Keep collection in the source inventory and derive the fixture
    # name from it; no unvalidated metadata is emitted here.
    fixtures = [{key: row[key] for key in ("id", "sha256", "writer", "policy")}
                for row in fixtures]
    cells = []
    for reader in readers:
        for fixture in fixtures:
            # Compatibility is an observation, never an inference from a
            # policy label. If a reader must refuse a shape, the fixture
            # command records that refusal and the reviewed plan is amended
            # with an explicit `refuse` cell after the run.
            cells.append({"id": f"{reader['id']}-{fixture['id']}",
                          "reader": reader["id"], "fixture": fixture["id"],
                          "expectation": "accept"})
    return {"schema": "cma-migration-matrix-plan-v1", "candidate": new,
            "readers": readers, "fixtures": fixtures, "cells": cells}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--inventory", type=Path, required=True)
    parser.add_argument("--old-source", required=True)
    parser.add_argument("--old-image", required=True)
    parser.add_argument("--new-source", required=True)
    parser.add_argument("--new-image", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args(argv)
    plan = build(json.loads(args.inventory.read_text()), args.old_source,
                 args.old_image, args.new_source, args.new_image)
    with args.output.open("x") as stream:
        stream.write(json.dumps(plan, indent=2) + "\n")
    print(json.dumps({"fixtures": len(plan["fixtures"]),
                      "cells": len(plan["cells"]),
                      "sha256": digest(args.output.read_bytes())}))


if __name__ == "__main__":
    main()
