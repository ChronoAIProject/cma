"""Resolve existing immutable versions for deployment without publication rights."""
import os
from pathlib import Path

from staging_cd_admission import event_identity, maintainer_dispatch
from staging_cd_common import Refusal, matches, require
from staging_cd_github import GitHub
from staging_cd_receipts import canonical, validate_receipt
from staging_cd_registry import Registry
from staging_cd_release_tags import local_tag_reader
from staging_cd_runtime import SKIPPED, run_private
from staging_cd_targets import RUNTIME_TARGETS, TARGETS
from staging_cd_versions import DEPLOYABLES, require_publishable_release, version_tuple
from staging_cd_watermark import record_withheld


class ObsoleteSource(Refusal):
    """Verified first-parent evidence identifies a newer published source."""


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
    if component in RUNTIME_TARGETS:
        newest = max(candidates, key=lambda item: history.position(item["target"]))
        if tag["target"] != newest["target"]:
            raise ObsoleteSource("Superseded runtime source cannot be promoted")
    receipt = tag["receipt"]
    require_publishable_release(receipt["identity"])
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
    if automatic or component in RUNTIME_TARGETS:
        latest = resolve(component, "latest", tags, registry, history)
        if latest["source"] != receipt["source"]:
            raise ObsoleteSource("Obsolete automatic deployment cannot replace a newer published source")
    return receipt


def output(env, name, value):
    require(matches(r"[a-z_]+", name) and isinstance(value, str) and "\n" not in value and "\r" not in value,
            "Deployment output is malformed")
    with open(env["GITHUB_OUTPUT"], "a", encoding="utf-8") as stream:
        stream.write(name + "=" + value + "\n")


def main():
    # Import here so the set adapter can reuse the single-receipt verifier.
    from staging_cd_admission_report import write_admission
    from staging_cd_clock import (
        dispatch_started,
        elapsed,
        remaining,
        run_created,
        timestamp,
    )
    from staging_cd_component_admission import automatic_admission, overtaken_admission, verified_resume
    from staging_cd_promotion import available_proofs
    from staging_cd_set import ALREADY_VERIFIED, OVERTAKEN, OvertakenByNewerFinal, dispatch_set, document, verify_set
    env = os.environ
    overtaken = []
    def operation(_masker):
        # Admission and verification re-read live refs, then their objects from the checkout.
        api, registry = GitHub(env.get("GH_TOKEN"), local_tags=local_tag_reader(Path.cwd())), Registry()
        event_identity(env)
        command = env.get("CD_RESOLVE_MODE")
        if command == "dispatch":
            inputs = maintainer_dispatch(env, api.request)
            started = dispatch_started(env, api.request, inputs.get("mode", "release"))
            mode, value = dispatch_set(inputs, api, registry)
            remaining(started, mode=mode)
        elif command == "automatic":
            require(env["GITHUB_EVENT_NAME"] == "push", "Automatic set requires a develop push")
            matrix = document(env["CD_PUBLICATION_MATRIX"])
            value, failures = automatic_admission(matrix, api, registry)
            if "CD_PROMOTION_LANE" in env:
                from staging_cd_routes import check_lane
                check_lane(env["CD_PROMOTION_LANE"], matrix, value, failures)
            output(env, "admitted", canonical(sorted(value["receipts"]) if value else []))
            output(env, "failures", canonical(failures))
            # The caller holds the shared promotion lock here. A writer that
            # was still active during allocation may now have an exact proof.
            already_verified = verified_resume(matrix, value, failures, api,
                                               lambda: available_proofs(env.get("GH_TOKEN"), env["CD_STAGING_ENVIRONMENT"]))
            output(env, "changed", "true" if value and not already_verified else "false")
            if already_verified:
                output(env, "disposition", ALREADY_VERIFIED)
                print("SKIP deployment reason=already-verified: a prior staging deployment verified this exact set")
            overtaken.extend(overtaken_admission(matrix, value, failures, api))
            if overtaken:
                # Deploy nothing; each newer final's own run promotes it.
                output(env, "disposition", OVERTAKEN)
                if env.get("GITHUB_STEP_SUMMARY"):
                    with open(env["GITHUB_STEP_SUMMARY"], "a", encoding="utf-8") as summary:
                        summary.write(f"Admission skipped: {OVERTAKEN} for {', '.join(overtaken)}. "
                                      "A newer final's run owns this deployment.\n")
            started = timestamp() if value else None
            selected = set(failures) | (set(value["receipts"]) if value else set())
            write_admission(env, selected, value, failures, started)
            if value is None or already_verified:
                return True
            publication_started = run_created(env, api.request)
            mode = "release"
            if env.get("GITHUB_STEP_SUMMARY"):
                with open(env["GITHUB_STEP_SUMMARY"], "a", encoding="utf-8") as stream:
                    stream.write(f"Publication-and-preparation seconds: {elapsed(publication_started):.2f}\n")
        elif command == "verify":
            started, mode = env.get("CD_STARTED_AT"), env.get("CD_DEPLOY_MODE")
            remaining(started, mode=mode)
            inputs = maintainer_dispatch(env, api.request) if env["GITHUB_EVENT_NAME"] == "workflow_dispatch" else None
            if inputs is not None:
                # A retry retains the run-created timestamp, never run_started_at.
                require(started == dispatch_started(env, api.request, mode), "Dispatch admission time changed")
                require(mode == inputs.get("mode", "release"), "Dispatch mode changed")
            else:
                require(mode == "release", "Automatic bootstrap is forbidden")
            admitted = document(env["CD_RELEASE_SET"])
            try:
                value = verify_set(admitted, api, registry, automatic=inputs is None, inputs=inputs)
            except OvertakenByNewerFinal as error:
                # Deploy nothing; each newer final's own run promotes it.
                overtaken.extend(error.components)
                output(env, "disposition", OVERTAKEN)
                if env.get("GITHUB_STEP_SUMMARY"):
                    with open(env["GITHUB_STEP_SUMMARY"], "a", encoding="utf-8") as summary:
                        summary.write(f"Deployment skipped: {OVERTAKEN} for {', '.join(error.components)}. "
                                      "A newer final's run owns this deployment.\n")
                return SKIPPED
            record_withheld(env, admitted["receipts"], value["receipts"])
            remaining(started, mode=mode)
        else:
            return False
        output(env, "release_set", canonical(value))
        output(env, "mode", mode)
        output(env, "started_at", started)
        return True
    status = run_private(operation, None, subject="release-set")
    if status == 0 and overtaken:
        # Fixed component tokens only; this run deploys nothing for them.
        stage = "admission" if env.get("CD_RESOLVE_MODE") == "automatic" else "deployment"
        print(f"SKIP {stage} reason={OVERTAKEN} components={','.join(overtaken)}: "
              "a newer final's run owns this deployment")
    return status


if __name__ == "__main__":
    raise SystemExit(main())
