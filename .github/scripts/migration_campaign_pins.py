#!/usr/bin/env python3
"""Reviewed, digest-pinned inputs for the CI migration campaign.

The pins name released Agent Manager readers by their version of record, the
git tag `cma-agent-manager/vX.Y.Z`, and by the exact GHCR manifest and config
digests that tag produced. `verify` refuses when either the git tag or the
registry tag no longer names the pinned identity, so the campaign never
follows a moved tag or a newer image. Local label and platform checks happen
after the pull, in the runner's own preflight.
"""
import argparse
import json
from pathlib import Path
import re
import subprocess
import sys

PINS = Path(__file__).with_name("migration-campaign-pins.json")
SCHEMA = "cma-migration-campaign-pins-v1"
REPOSITORY = "ghcr.io/chronoaiproject/cma-agent-manager"
RELEASE = re.compile(r"^cma-agent-manager/v(?P<version>[0-9]+\.[0-9]+\.[0-9]+)$")
DIGEST = re.compile(r"^sha256:[a-f0-9]{64}$")
SOURCE = re.compile(r"^[a-f0-9]{40}$")
KIND_VERSION = re.compile(r"^v[0-9]+\.[0-9]+\.[0-9]+$")
HEX = re.compile(r"^[a-f0-9]{64}$")
NODE = re.compile(r"^docker\.io/kindest/node@sha256:[a-f0-9]{64}$")
MONGO = re.compile(r"^docker\.io/library/mongo@sha256:[a-f0-9]{64}$")
ROLES = ("old", "new")


class PinError(ValueError):
    """A named refusal; `stage` tells the owner which check failed."""

    def __init__(self, stage, message):
        super().__init__(message)
        self.stage = stage


def fields(value, names, where):
    if not isinstance(value, dict) or set(value) != set(names):
        raise PinError("pins", f"{where} has missing or unknown fields")


def load(path=PINS):
    try:
        document = json.loads(Path(path).read_text())
    except (OSError, json.JSONDecodeError) as error:
        raise PinError("pins", "campaign pins are missing or not JSON") from error
    fields(document, ("schema", "readers", "kind", "mongo", "identity"), "pins")
    if document["schema"] != SCHEMA:
        raise PinError("pins", "campaign pins schema is not " + SCHEMA)
    readers = document["readers"]
    if not isinstance(readers, list) or [row.get("role") if isinstance(row, dict) else None
                                         for row in readers] != list(ROLES):
        raise PinError("pins", "readers must be exactly old then new")
    for row in readers:
        fields(row, ("role", "release", "image", "config", "source"), "reader " + row["role"])
        if not isinstance(row["release"], str) or not RELEASE.fullmatch(row["release"]):
            raise PinError("pins", f"{row['role']} release must be a cma-agent-manager version tag")
        if not isinstance(row["image"], str) or not row["image"].startswith(REPOSITORY + "@") \
                or not DIGEST.fullmatch(row["image"].split("@", 1)[1]):
            raise PinError("pins", f"{row['role']} image must be {REPOSITORY} pinned by digest")
        if not isinstance(row["config"], str) or not DIGEST.fullmatch(row["config"]):
            raise PinError("pins", f"{row['role']} config must be a sha256 digest")
        if not isinstance(row["source"], str) or not SOURCE.fullmatch(row["source"]):
            raise PinError("pins", f"{row['role']} source must be a full commit SHA")
    if readers[0]["release"] == readers[1]["release"] or readers[0]["image"] == readers[1]["image"]:
        raise PinError("pins", "old and new readers must be different releases")
    kind = document["kind"]
    fields(kind, ("version", "linux_amd64_sha256", "node"), "kind")
    if not isinstance(kind["version"], str) or not KIND_VERSION.fullmatch(kind["version"]):
        raise PinError("pins", "kind version must be an exact release")
    if not isinstance(kind["linux_amd64_sha256"], str) or not HEX.fullmatch(kind["linux_amd64_sha256"]):
        raise PinError("pins", "kind binary must be pinned by sha256")
    if not isinstance(kind["node"], str) or not NODE.fullmatch(kind["node"]):
        raise PinError("pins", "kind node image must be pinned by digest")
    if not isinstance(document["mongo"], str) or not MONGO.fullmatch(document["mongo"]):
        raise PinError("pins", "Mongo image must be pinned by digest")
    from migration_kind_manifests import IDENTITY
    if document["identity"] != IDENTITY:
        raise PinError("pins", "identity provider image must be the reviewed runner image")
    return document


def git_tag_commit(release):
    """The commit the release tag names on origin, peeling an annotated tag."""
    output = subprocess.check_output(
        ["git", "ls-remote", "--tags", "origin", f"refs/tags/{release}", f"refs/tags/{release}^{{}}"],
        text=True, timeout=60)
    refs = {ref: sha for sha, ref in (line.split("\t", 1) for line in output.splitlines() if "\t" in line)}
    return refs.get(f"refs/tags/{release}^{{}}") or refs.get(f"refs/tags/{release}")


def verify(document, registry, tag_commit=git_tag_commit):
    """Both tags of record must still name each pinned reader exactly."""
    for row in document["readers"]:
        version = RELEASE.fullmatch(row["release"]).group("version")
        try:
            commit = tag_commit(row["release"])
        except (subprocess.SubprocessError, OSError) as error:
            raise PinError("git-tag", f"{row['release']} could not be read from origin") from error
        if commit != row["source"]:
            raise PinError("git-tag", f"{row['release']} does not name the pinned source")
        try:
            result = registry.manifest(REPOSITORY, version)
        except Exception as error:  # the registry raises its own named refusals
            raise PinError("registry", f"{row['release']} manifest could not be read") from error
        if result is None:
            raise PinError("registry", f"{row['release']} is absent from {REPOSITORY}")
        digest, manifest = result
        if digest != row["image"].split("@", 1)[1]:
            raise PinError("registry", f"{row['release']} no longer names the pinned digest")
        if (manifest.get("config") or {}).get("digest") != row["config"]:
            raise PinError("registry", f"{row['release']} manifest config differs from the pin")
    return [row["release"] for row in document["readers"]]


def environment(document):
    """KEY=value lines for $GITHUB_ENV; every value is a validated identity."""
    old, new = document["readers"]
    rows = {
        "CAMPAIGN_OLD_IMAGE": old["image"], "CAMPAIGN_OLD_SOURCE": old["source"],
        "CAMPAIGN_NEW_IMAGE": new["image"], "CAMPAIGN_NEW_SOURCE": new["source"],
        "CAMPAIGN_KIND_VERSION": document["kind"]["version"],
        "CAMPAIGN_KIND_SHA256": document["kind"]["linux_amd64_sha256"],
        "CAMPAIGN_KIND_NODE": document["kind"]["node"],
        "CAMPAIGN_MONGO_IMAGE": document["mongo"],
        "CAMPAIGN_IDENTITY_IMAGE": document["identity"],
    }
    return "".join(f"{key}={value}\n" for key, value in rows.items())


def images(document):
    return [row["image"] for row in document["readers"]] + [
        document["mongo"], document["identity"], document["kind"]["node"]]


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("check", "verify", "env", "images"))
    parser.add_argument("--pins", type=Path, default=PINS)
    args = parser.parse_args(argv)
    try:
        document = load(args.pins)
        if args.command == "verify":
            from staging_cd_registry import Registry
            for release in verify(document, Registry()):
                print(f"{release}: git tag and registry tag name the pinned reader")
        elif args.command == "env":
            sys.stdout.write(environment(document))
        elif args.command == "images":
            sys.stdout.write("".join(image + "\n" for image in images(document)))
        else:
            print("campaign pins are well-formed")
    except PinError as error:
        print(f"::error title=Migration campaign {error.stage}::{error}. "
              "Review .github/scripts/migration-campaign-pins.json against the release tags.",
              file=sys.stderr)
        raise SystemExit(1) from error


if __name__ == "__main__":
    main()
