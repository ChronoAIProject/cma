"""Resolve existing immutable versions for deployment without publication rights."""
import os

from staging_cd_admission import event_identity, maintainer_dispatch
from staging_cd_common import matches, require
from staging_cd_github import GitHub
from staging_cd_receipts import canonical, validate_receipt
from staging_cd_registry import Registry
from staging_cd_runtime import run_private
from staging_cd_versions import DEPLOYABLES, version_tuple
from staging_cd_targets import TARGETS


def components(value):
    require(isinstance(value, str), "Redeployment components must be comma-separated")
    selected = value.split(",")
    require(1 <= len(selected) <= len(TARGETS) and len(set(selected)) == len(selected)
            and all(item in TARGETS for item in selected),
            "Redeployment components are invalid or repeated")
    return sorted(selected)


def resolve(component, version, tags, registry, history):
    require(component in TARGETS, "Redeployment component is not allowed")
    prefix = DEPLOYABLES[component] + "/v"
    candidates = [tag for tag in tags if tag["name"].startswith(prefix)]
    require(bool(candidates), "No published component version exists")
    for tag in candidates:
        receipt = validate_receipt(tag["receipt"])
        require(receipt["tag"] == tag["name"] and receipt["source"] == tag["target"],
                "Published version target differs")
        history.position(receipt["source"])
    require(len({tag["target"] for tag in candidates}) == len(candidates),
            "Published component sources are duplicated")
    if version in ("", "latest"):
        tag = max(candidates, key=lambda item: history.position(item["target"]))
    else:
        version_tuple(version)
        selected = [tag for tag in candidates if tag["name"] == prefix + version]
        require(len(selected) == 1, "Requested published version is absent or ambiguous")
        tag = selected[0]
    receipt = tag["receipt"]
    require(registry.exists(receipt["repository"], receipt["digest"]) is True,
            "Published digest is not publicly available")
    version_manifest = registry.manifest(receipt["repository"], receipt["identity"]["version"])
    require(version_manifest is not None and version_manifest[0] == receipt["digest"],
            "Published version manifest differs from the immutable receipt")
    return receipt


def verify(receipt, tags, registry, history, *, automatic):
    validate_receipt(receipt)
    component = receipt["identity"]["component"]
    expected = resolve(component, receipt["identity"]["version"], tags, registry, history)
    require(expected == receipt, "Deployment receipt no longer matches the immutable version")
    if automatic:
        latest = resolve(component, "latest", tags, registry, history)
        require(latest["source"] == receipt["source"],
                "Obsolete automatic deployment cannot replace a newer published source")
    return receipt


def output(env, name, value):
    require(matches(r"[a-z_]+", name) and isinstance(value, str) and "\n" not in value and "\r" not in value,
            "Deployment output is malformed")
    with open(env["GITHUB_OUTPUT"], "a", encoding="utf-8") as stream:
        stream.write(name + "=" + value + "\n")


def main():
    # Import here so the set adapter can reuse the single-receipt verifier.
    from staging_cd_clock import dispatch_started, elapsed, remaining, run_created, timestamp
    from staging_cd_set import automatic_set, dispatch_set, document, verify_set
    env = os.environ
    def operation(_masker):
        api, registry = GitHub(env.get("GH_TOKEN")), Registry()
        event_identity(env)
        command = env.get("CD_RESOLVE_MODE")
        if command == "dispatch":
            inputs = maintainer_dispatch(env, api.request)
            started = dispatch_started(env, api.request)
            mode, value = dispatch_set(inputs, api, registry)
            remaining(started)
        elif command == "automatic":
            require(env["GITHUB_EVENT_NAME"] == "push", "Automatic set requires a develop push")
            value = automatic_set(document(env["CD_PUBLICATION_MATRIX"]), api, registry)
            output(env, "changed", "true" if value else "false")
            if value is None:
                return True
            publication_started = run_created(env, api.request)
            mode, started = "release", timestamp()
            if env.get("GITHUB_STEP_SUMMARY"):
                with open(env["GITHUB_STEP_SUMMARY"], "a", encoding="utf-8") as stream:
                    stream.write(f"Publication-and-preparation seconds: {elapsed(publication_started):.2f}\n")
        elif command == "verify":
            started, mode = env.get("CD_STARTED_AT"), env.get("CD_DEPLOY_MODE")
            remaining(started)
            inputs = maintainer_dispatch(env, api.request) if env["GITHUB_EVENT_NAME"] == "workflow_dispatch" else None
            if inputs is not None:
                # A retry retains the run-created timestamp, never run_started_at.
                require(started == dispatch_started(env, api.request), "Dispatch admission time changed")
                require(mode == inputs.get("mode", "release"), "Dispatch mode changed")
            else:
                require(mode == "release", "Automatic bootstrap is forbidden")
            value = verify_set(document(env["CD_RELEASE_SET"]), api, registry,
                               automatic=inputs is None, inputs=inputs)
            remaining(started)
        else:
            return False
        output(env, "release_set", canonical(value))
        output(env, "mode", mode)
        output(env, "started_at", started)
        return True
    return run_private(operation, None)


if __name__ == "__main__":
    raise SystemExit(main())
