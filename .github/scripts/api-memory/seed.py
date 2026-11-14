#!/usr/bin/env python3
"""Restore typed synthetic BSON through mongosh in the owned disposable container."""
import argparse
import json
from pathlib import Path
import re
import subprocess
from urllib.parse import urlsplit

LABEL = "dev.chrono.api-memory-rca"
RESTORE = r'''
const input = JSON.parse(require('fs').readFileSync(0, 'utf8'));
const data = EJSON.parse(input.fixture, {relaxed: false});
const name = input.database;
if (!/^cma_memory_rca_[a-z0-9_]{1,32}$/.test(name)) throw Error('invalid scope');
if (!db.hello().setName) throw Error('replica set required');
if (db.getSiblingDB('admin').runCommand({listDatabases: 1, nameOnly: true}).databases.some(x => x.name === name)) throw Error('database already exists');
const target = db.getSiblingDB(name);
for (const [collection, rows] of Object.entries(data.collections)) {
    target.getCollection(collection).insertMany(rows, {ordered: true});
    if (target.getCollection(collection).countDocuments({}) !== rows.length) throw Error('count mismatch');
}
print(JSON.stringify({outcome: 'seeded', agents: 1, turns: 128, events: 515, pending_work: 0}));
'''


def docker(*args, input=None, timeout=60):
    return subprocess.check_output(["docker", "--context", "desktop-linux", *args],
                                   input=input, text=True, stderr=subprocess.PIPE, timeout=timeout)


def seed(fixture, uri, database, container=None):
    endpoint = urlsplit(uri)
    if (endpoint.scheme != "mongodb" or endpoint.hostname != "127.0.0.1"
            or not endpoint.port or endpoint.username or endpoint.password
            or "," in endpoint.netloc or endpoint.path not in ("", "/")
            or endpoint.query or endpoint.fragment):
        raise ValueError("disposable loopback Mongo replica set required")
    if not re.fullmatch(r"cma_memory_rca_[a-z0-9_]{1,32}", database):
        raise ValueError("dedicated RCA database name required")
    if fixture.is_symlink() or not fixture.is_file() or fixture.stat().st_mode & 0o077:
        raise ValueError("private regular fixture required")
    if not container or not re.fullmatch(r"cma-memory-rca-[a-z0-9-]+-mongo", container):
        raise ValueError("owned disposable Mongo container required")
    owner = docker("inspect", "--format", '{{index .Config.Labels "' + LABEL + '"}}', container).strip()
    if owner != container.removesuffix("-mongo"):
        raise ValueError("Mongo ownership label mismatch")
    raw = fixture.read_text()
    data = json.loads(raw)
    expected = {"sandboxes": 1, "workspaces": 1, "worktrees": 1, "agents": 1, "agent_log": 515}
    if (data.get("environment") != "isolated-local" or data.get("synthetic_identity") is not True
            or data.get("head") != 515 or set(data["collections"]) != set(expected)
            or any(len(data["collections"][key]) != count for key, count in expected.items())):
        raise ValueError("fixed typed fixture required")
    result = docker("exec", "-i", container, "mongosh", "--quiet", "--eval", RESTORE,
                    input=json.dumps({"fixture": raw, "database": database}))
    if json.loads(result).get("outcome") != "seeded":
        raise RuntimeError("fixture restore did not acknowledge completion")
    print('{"outcome":"seeded","agents":1,"turns":128,"events":515,"pending_work":0}')


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("fixture", type=Path)
    parser.add_argument("uri")
    parser.add_argument("database")
    parser.add_argument("--container", required=True)
    args = parser.parse_args()
    try:
        seed(args.fixture, args.uri, args.database, args.container)
    except Exception as error:
        print(json.dumps({"outcome": "incomplete", "failure_type": type(error).__name__}))
        raise SystemExit(1)
