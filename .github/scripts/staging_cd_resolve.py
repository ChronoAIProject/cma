"""Resolve existing immutable versions for deployment without publication rights."""
import json
import os

from staging_cd_admission import event_identity, maintainer_dispatch
from staging_cd_common import matches, require
from staging_cd_github import GitHub
from staging_cd_receipts import canonical, validate_receipt
from staging_cd_registry import Registry
from staging_cd_runtime import run_private
from staging_cd_versions import DEPLOYABLES, version_tuple


def components(value):
    require(isinstance(value, str), "Redeployment components must be comma-separated")
    selected = value.split(",")
    require(1 <= len(selected) <= 3 and len(set(selected)) == len(selected)
            and all(item in ("am", "eg", "web") for item in selected),
            "Redeployment components are invalid or repeated")
    return sorted(selected)


def resolve(component, version, tags, registry):
    require(component in ("am", "eg", "web"), "Redeployment component is not allowed")
    prefix = DEPLOYABLES[component] + "/v"
    candidates = [tag for tag in tags if tag["name"].startswith(prefix)]
    require(bool(candidates), "No published component version exists")
    for tag in candidates:
        receipt = validate_receipt(tag["receipt"])
        require(receipt["tag"] == tag["name"] and receipt["source"] == tag["target"],
                "Published version target differs")
    if version in ("", "latest"):
        tag = max(candidates, key=lambda item: version_tuple(item["name"][len(prefix):]))
    else:
        version_tuple(version)
        selected = [tag for tag in candidates if tag["name"] == prefix + version]
        require(len(selected) == 1, "Requested published version is absent or ambiguous")
        tag = selected[0]
    receipt = tag["receipt"]
    require(registry.exists(receipt["repository"], receipt["digest"]) is True,
            "Published digest is not publicly available")
    return receipt


def verify(receipt, tags, registry, *, automatic):
    validate_receipt(receipt)
    component = receipt["identity"]["component"]
    expected = resolve(component, receipt["identity"]["version"], tags, registry)
    require(expected == receipt, "Deployment receipt no longer matches the immutable version")
    if automatic:
        latest = resolve(component, "latest", tags, registry)
        require(latest["identity"]["version"] == receipt["identity"]["version"],
                "Obsolete automatic deployment cannot replace a newer published version")
    return receipt


def output(env, name, value):
    require(matches(r"[a-z_]+", name) and isinstance(value, str) and "\n" not in value and "\r" not in value,
            "Deployment output is malformed")
    with open(env["GITHUB_OUTPUT"], "a", encoding="utf-8") as stream:
        stream.write(name + "=" + value + "\n")


def main():
    env = os.environ
    def operation(_masker):
        api, registry = GitHub(env.get("GH_TOKEN")), Registry()
        event_identity(env)
        if env.get("CD_RESOLVE_MODE") == "dispatch":
            inputs = maintainer_dispatch(env, api.request)
            matrix = [{"component": component, "receipt": canonical(resolve(component, inputs.get("version", ""),
                                                                         api.tags(component), registry))}
                      for component in components(inputs.get("deployables"))]
            output(env, "matrix", canonical({"include": matrix}))
            return True
        elif env.get("CD_RESOLVE_MODE") == "verify":
            if env["GITHUB_EVENT_NAME"] == "workflow_dispatch":
                inputs = maintainer_dispatch(env, api.request)
            receipt = json.loads(env["CD_RECEIPT"])
            validate_receipt(receipt)
            component = receipt["identity"]["component"]
            require(component == env.get("CD_COMPONENT_EXPECTED"), "Deployment group does not match the receipt")
            if env["GITHUB_EVENT_NAME"] == "workflow_dispatch":
                require(component in components(inputs.get("deployables")), "Dispatch component differs")
            verify(receipt, api.tags(component), registry, automatic=env["GITHUB_EVENT_NAME"] == "push")
        else:
            return False
        output(env, "receipt", canonical(receipt))
        output(env, "component", component)
        output(env, "image", receipt["repository"] + "@" + receipt["digest"])
        output(env, "source", receipt["source"])
        return True
    return run_private(operation, None)


if __name__ == "__main__":
    raise SystemExit(main())
