"""Use the exact finalized staging image for develop-push CI qualification."""

import json
import os
import re
import subprocess
import sys
import time
from pathlib import Path

import ci_retry
from staging_cd_admission import event_identity
from staging_cd_build import identity
from staging_cd_common import Refusal, matches, require
from staging_cd_contract import from_source, verify_labels
from staging_cd_github import GitHub
from staging_cd_receipts import IDENTITY_LABEL, canonical, final_tag_oid, parse_annotation
from staging_cd_registry import MEDIA, Registry
from staging_cd_release_tags import REMOTE, local_tag_reader
from staging_cd_resolve import output as step_output
from staging_cd_runtime import run_private
from staging_cd_versions import DEPLOYABLES, REPOSITORY, SEMVER

COMPONENTS = {"api": "am", "runtime": "runtime", "runtime-omarchy": "runtime-omarchy"}
WAIT_SECONDS = 23 * 60
POLL_SECONDS = 30
PULL_SECONDS = 5 * 60
SHA256 = r"sha256:[0-9a-f]{64}"


def checked_command(args, *, timeout, runner=subprocess.run):
    result = runner(args, timeout=timeout, capture_output=True, text=True, check=False)
    if result.returncode:
        raise Refusal("Image qualification command failed") from ci_retry.cli_cause(result.stderr)
    return result.stdout


def refs(component, source, run=checked_command, *, timeout=20):
    prefix = "refs/tags/" + DEPLOYABLES[component] + "/v"
    raw = run(["git", "ls-remote", "--tags", REMOTE, prefix + "*"], timeout=timeout)
    tags, peeled = {}, []
    for line in raw.splitlines():
        oid, sep, ref = line.partition("\t")
        name = ref.removesuffix("^{}")
        require(sep and matches(r"[0-9a-f]{40}", oid)
                and matches(re.escape(prefix) + SEMVER, name), "Staging final ref inventory is malformed")
        if name != ref:
            if oid == source:
                peeled.append(name)
        else:
            require(oid != source, "Staging final on the CI source must be an annotated tag")
            require(ref not in tags, "Staging final ref appears twice")
            tags[ref] = oid
    require(len(peeled) <= 1, "Multiple staging finals name the same component and source")
    if not peeled:
        return None
    require(peeled[0] in tags, "Staging final is missing its annotated object")
    ref = peeled[0]
    return ref, tags[ref]


def matching_receipt(ref, oid, component, source, tree, api):
    tag = api.request("git/tags/" + oid)
    require(isinstance(tag, dict) and tag.get("sha") == oid and tag.get("tag") == ref[len("refs/tags/"):],
            "Staging final tag identity differs")
    target = tag.get("object")
    require(isinstance(target, dict) and target.get("type") == "commit" and target.get("sha") == source,
            "Staging final tag target differs")
    receipt = parse_annotation(tag.get("message"))
    release = receipt["identity"]
    if component in ("runtime", "runtime-omarchy"):
        require("runtime_smoke" in receipt, "Staging runtime final lacks smoke evidence")
    require(oid == final_tag_oid(receipt) and ref == "refs/tags/" + receipt["tag"]
            and release["component"] == component and receipt["source"] == source
            and release["tree"] == tree, "Staging final receipt differs from CI source")
    run = api.request(f"actions/runs/{release['run_id']}/attempts/{release['run_attempt']}")
    require(isinstance(run, dict) and run.get("id") == release["run_id"]
            and run.get("run_attempt") == release["run_attempt"] and run.get("event") == "push"
            and run.get("head_branch") == "develop" and matches(r"[0-9a-f]{40}", run.get("head_sha"))
            and run.get("path") == ".github/workflows/staging-cd.yml"
            and (run.get("repository") or {}).get("full_name") == REPOSITORY
            and (run.get("head_repository") or {}).get("full_name") == REPOSITORY,
            "Staging final names a different workflow")
    # Staging can publish an earlier source while draining a release backlog.
    # Its valid final is not a build by this head's staging run.
    run_head = run["head_sha"]
    if run_head == source:
        return receipt
    history = api.history_containing(run_head)
    require(history.position(source) < history.position(run_head),
            "Staging final source does not precede its owning run")
    return None


def await_ref(component, source, *, lookup=refs, clock=time.monotonic, sleep=time.sleep):
    deadline = clock() + WAIT_SECONDS
    last_miss = "no-final"
    while clock() < deadline:
        remaining = deadline - clock()
        if remaining < 1:
            break
        try:
            found = lookup(component, source, timeout=min(20, remaining))
        except Refusal as error:
            if ci_retry.classify(error) != ci_retry.TRANSIENT:
                raise
            last_miss = "unreachable"
        except (TimeoutError, subprocess.TimeoutExpired):
            last_miss = "unreachable"
        else:
            if found is not None:
                return found, None
            last_miss = "no-final"
        sleep(min(POLL_SECONDS, max(0, deadline - clock())))
    return None, last_miss


def validate_loaded(row, reference, config_digest, expected, compatibility):
    manifest_digest = reference.split("@", 1)[1]
    descriptor = row.get("Descriptor")
    require(reference in (row.get("RepoDigests") or []), "Docker did not load the recorded manifest")
    if descriptor is None:
        require(row.get("Id") == config_digest, "Docker image config differs from the staging manifest")
    else:
        require(isinstance(descriptor, dict) and descriptor.get("digest") == manifest_digest
                and descriptor.get("mediaType") in MEDIA.split(", ")
                and row.get("Id") in (manifest_digest, config_digest),
                "Docker image descriptor differs from the staging manifest")
        annotations = descriptor.get("annotations") or {}
        require(annotations.get("config.digest", config_digest) == config_digest,
                "Docker image config differs from the staging manifest")
    config = row.get("Config") or {}
    labels = config.get("Labels") or {}
    require(row.get("Architecture") == "amd64" and row.get("Os") == "linux"
            and labels.get(IDENTITY_LABEL) == canonical(expected)
            and labels.get("org.opencontainers.image.revision") == expected["source"]
            and labels.get("org.opencontainers.image.version") == expected["version"],
            "Docker image differs from the reserved release identity")
    verify_labels(labels, compatibility, expected["component"])


def adopt(receipt, target, root, registry, run=checked_command):
    release = receipt["identity"]
    reservation = {key: value for key, value in release.items() if key != "build"}
    reservation["schema"] = "cma-version-reservation-v1"
    expected, _, _ = identity(reservation, root)
    compatibility = from_source(release["component"], release["source"], root)
    require(expected == release and compatibility is not None
            and receipt.get("compatibility") == compatibility, "Staging release recipe differs from CI source")

    reference = receipt["repository"] + "@" + receipt["digest"]
    result = registry.manifest(receipt["repository"], receipt["digest"], absent=True)
    if result is None:
        return None
    digest, manifest = result
    config_digest = manifest["config"].get("digest")
    require(digest == receipt["digest"] and matches(SHA256, config_digest), "Staging manifest digest differs")
    run(["docker", "pull", "--platform", "linux/amd64", reference], timeout=PULL_SECONDS)
    rows = json.loads(run(["docker", "image", "inspect", reference], timeout=20))
    require(isinstance(rows, list) and len(rows) == 1 and isinstance(rows[0], dict),
            "Docker image lookup did not return one image")
    validate_loaded(rows[0], reference, config_digest, expected, compatibility)
    run(["docker", "tag", reference, target], timeout=20)
    tagged = json.loads(run(["docker", "image", "inspect", target], timeout=20))
    require(isinstance(tagged, list) and len(tagged) == 1 and tagged[0].get("Id") == rows[0]["Id"],
            "CI image tag does not identify the staging image")
    return digest


def qualification(env, root):
    event_identity(env)
    require(env["GITHUB_EVENT_NAME"] == "push" and env["IMAGE_COMPONENT"] in COMPONENTS,
            "Staging image qualification requires a develop image lane")
    source = env["GITHUB_SHA"]
    require(checked_command(["git", "rev-parse", "HEAD"], timeout=10).strip() == source,
            "CI checkout differs from the develop push")
    tree = checked_command(["git", "rev-parse", "HEAD^{tree}"], timeout=10).strip()
    component = COMPONENTS[env["IMAGE_COMPONENT"]]
    found, miss = await_ref(component, source)
    if found is None:
        return None, miss
    try:
        api = GitHub(env.get("GH_TOKEN"), local_tags=local_tag_reader(root), deadline=time.monotonic() + 90)
        receipt = matching_receipt(*found, component, source, tree, api)
        if receipt is None:
            return None, "historical-final"
        target = DEPLOYABLES[component] + ":ci"
        digest = adopt(receipt, target, root, Registry(deadline=time.monotonic() + 90))
    except (Refusal, subprocess.TimeoutExpired) as error:
        if ci_retry.classify(error) != ci_retry.TRANSIENT:
            raise
        return None, "unreachable"
    return (digest, None) if digest is not None else (None, "registry-unavailable")


def main():
    result = []

    def run(_masker):
        result.append(qualification(os.environ, Path.cwd()))
        return True

    code = run_private(run, None, subject="staging-ci-image")
    if result:
        digest, miss = result[0]
        if digest is not None:
            step_output(os.environ, "adopted", "true")
            print("CI image source=staging digest=" + digest, flush=True)
        else:
            print("CI staging image reuse miss reason=" + miss, flush=True)
    return code


if __name__ == "__main__":
    sys.exit(main())
