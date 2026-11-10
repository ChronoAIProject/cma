#!/usr/bin/env python3
"""Guard the operator apply path; live owner-key values never leave Kubernetes."""

import argparse
import copy
import json
from pathlib import Path
import subprocess
import sys


OWNER_KEYS = ("CMAEG_OWNER_KEY_SERVICE_ID", "CMAEG_OWNER_KEY_CUTOVER_ID")
NAMESPACE = "chrono-bot"
NAME = "cma-event-gateway"


class Refused(RuntimeError):
    """A sanitized refusal, safe to display without subprocess output."""


def invoke(args, input_text=None):
    try:
        result = subprocess.run(
            args, input=input_text, text=True, capture_output=True, timeout=45,
            check=False,
        )
    except (OSError, subprocess.SubprocessError):
        raise Refused("command failed or timed out; no further apply attempted") from None
    if result.returncode:
        raise Refused("command refused; no further apply attempted")
    return result.stdout


def load_manifests():
    root = Path(__file__).resolve().parents[2]
    parser = (
        'require "yaml"; require "json"; '
        'puts JSON.generate(YAML.safe_load(STDIN.read, '
        'permitted_classes: [], aliases: false))'
    )
    try:
        return [json.loads(invoke(
            ["ruby", "-e", parser],
            (root / "infra" / "cmaeg" / filename).read_text(),
        )) for filename in ("configmap.yaml", "deployment.yaml")]
    except (OSError, ValueError):
        raise Refused("cannot parse the fixed CMAEG manifests") from None


def projection(kind):
    # Kubernetes formats presence flags, never the owner-key values themselves.
    fields = [
        '"' + field + '":{{printf "%q" .metadata.' + field + '}}'
        for field in ("name", "namespace", "uid", "resourceVersion")
    ]
    fields.append('"deleting":{{if .metadata.deletionTimestamp}}true{{else}}false{{end}}')
    if kind == "ConfigMap":
        fields.append('"nonempty":{' + ",".join(
            '"' + key + '":{{if index .data "' + key + '"}}true{{else}}false{{end}}'
            for key in OWNER_KEYS
        ) + "}")
    return "{" + ",".join(fields) + "}"


def validate_manifest(document, kind):
    if not isinstance(document, dict):
        raise Refused("invalid manifest object")
    metadata = document.get("metadata", {})
    api_version = "v1" if kind == "ConfigMap" else "apps/v1"
    if (document.get("kind") != kind or document.get("apiVersion") != api_version
            or not isinstance(metadata, dict)
            or metadata.get("name") != NAME or metadata.get("namespace") != NAMESPACE):
        raise Refused("unexpected CMAEG manifest identity")
    if kind == "ConfigMap":
        data = document.get("data")
        if not isinstance(data, dict) or not all(isinstance(v, str) for v in data.values()):
            raise Refused("ConfigMap data must contain string values")
        if any(key not in data for key in OWNER_KEYS):
            raise Refused("owner-key placeholders must remain explicit until the Secret cutover")


def read_live(base, kind, command):
    try:
        live = json.loads(command(base + [
            "get", kind.lower(), NAME, "-o", "go-template=" + projection(kind),
        ]))
        valid = (
            live.get("name") == NAME and live.get("namespace") == NAMESPACE
            and live.get("deleting") is False
            and all(isinstance(live.get(key), str) and live[key]
                    for key in ("uid", "resourceVersion"))
        )
        if kind == "ConfigMap":
            valid = valid and all(
                type(live.get("nonempty", {}).get(key)) is bool for key in OWNER_KEYS
            )
        if not valid:
            raise ValueError()
        return live
    except (ValueError, AttributeError, TypeError):
        raise Refused("live identity or owner-key presence could not be verified") from None


def guarded_apply(documents, context, mode="check", command=invoke):
    if not context or context.startswith("-") or any(c.isspace() for c in context):
        raise Refused("an explicit Kubernetes context is required")
    if mode not in ("check", "dry-run", "apply") or len(documents) != 2:
        raise Refused("invalid apply mode or manifest set")
    desired = copy.deepcopy(documents)
    for document, kind in zip(desired, ("ConfigMap", "Deployment")):
        validate_manifest(document, kind)
    base = ["kubectl", "--context", context, "--namespace", NAMESPACE, "--request-timeout=30s"]
    snapshots = [read_live(base, kind, command) for kind in ("ConfigMap", "Deployment")]
    for key in OWNER_KEYS:
        if not desired[0]["data"][key].strip() and snapshots[0]["nonempty"][key]:
            raise Refused("empty placeholder would erase live " + key + "; apply refused")
    if mode == "check":
        return
    for document, live in zip(desired, snapshots):
        # SSA sends these preconditions on the write itself, closing the GET/write race.
        # No force-conflicts: an ownership conflict requires operator review.
        document["metadata"].update({key: live[key] for key in ("uid", "resourceVersion")})
        args = base + ["apply", "--server-side", "--field-manager=cmaeg-operations",
                       "--validate=strict", "-f", "-", "-o", "name"]
        if mode == "dry-run":
            args.append("--dry-run=server")
        command(args, json.dumps(document))


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--context", required=True)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--check", action="store_true", help="bounded GETs only")
    mode.add_argument("--dry-run", choices=["server"])
    mode.add_argument("--apply", action="store_true")
    options = parser.parse_args(argv)
    selected = "apply" if options.apply else "dry-run" if options.dry_run else "check"
    try:
        guarded_apply(load_manifests(), options.context, selected)
    except Refused as error:
        print("CMAEG apply refused: " + str(error), file=sys.stderr)
        return 1
    print("CMAEG guarded " + selected + " passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
