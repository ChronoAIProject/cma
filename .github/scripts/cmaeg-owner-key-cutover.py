#!/usr/bin/env python3
"""Train operator only. Bounded Kubernetes reads; secret-bearing writes use stdin."""
import argparse
import json
from pathlib import Path
import subprocess
import sys

import cmaeg_owner_key_cutover as model


class Kubectl:
    def __init__(self, context):
        self.context = context

    def call(self, arguments, payload=None):
        # No shell, argument values, stderr, response objects or last-applied copies.
        command = ["kubectl", "--context", self.context, "--namespace", model.NAMESPACE,
                   "--request-timeout=10s", "--v=0"] + arguments
        try:
            result = subprocess.run(command, input=None if payload is None else json.dumps(payload).encode(),
                                    stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, timeout=20, check=False)
        except (OSError, subprocess.TimeoutExpired):
            raise model.Refused("kubernetes_request_failed_or_uncertain") from None
        model.require(result.returncode == 0, "kubernetes_request_failed_or_uncertain")
        model.require(len(result.stdout) <= 2 * 1024 * 1024, "kubernetes_response_too_large")
        return result.stdout

    def get(self, kind, name):
        raw = self.call(["get", kind, name, "--ignore-not-found", "-o", "json"])
        return json.loads(raw) if raw.strip() else None

    def read(self):
        return (self.get("configmap", model.NAME), self.get("deployment", model.NAME),
                self.get("secret", model.SECRET))

    def create_secret(self, body):
        self.call(["create", "-f", "-", "-o", "name"], body)

    def patch(self, kind, name, patch):
        self.call(["patch", kind, name, "--type=json", "--patch-file=/dev/stdin", "-o", "name"], patch)


def unchanged(before, after, indices, context):
    old, new = model.projection(before, context), model.projection(after, context)
    for field in indices:
        model.require(old[field] == new[field], "dependency_changed_hold")


def execute(client, phase, expected=None, receipt=None):
    before = client.read()
    fence = model.projection(before, client.context)
    if phase != "capture":
        model.require(expected == fence, "capture_fence_changed")
    cm, dep, secret = before
    if phase == "capture":
        return fence
    if phase == "verify":
        model.require(model.reference_state(dep) and model.ready(dep)
                      and model.config_pair(cm) is None and secret is not None, "cutover_not_verified")
        model.require(not any(key in cm["metadata"].get("annotations", {}).get(model.LAST_APPLIED, "")
                              for key in model.KEYS), "legacy_annotation_remaining")
        return fence
    # A shared release lock and freeze of other writers are required as well:
    # Kubernetes cannot atomically CAS a Secret, ConfigMap and Deployment together.
    if phase == "seed":
        body = model.seed_plan(cm, secret)
        if body is not None:
            unchanged(before, client.read(), ("configmap", "deployment", "secret"), client.context)
            client.create_secret(body)
        after = client.read()
        unchanged(before, after, ("configmap", "deployment"), client.context)
        model.same_values(after[0], after[2])
    elif phase == "switch":
        patch = model.switch_plan(cm, dep, secret)
        if patch is not None:
            unchanged(before, client.read(), ("configmap", "deployment", "secret"), client.context)
            client.patch("deployment", model.NAME, patch)
        after = client.read()
        unchanged(before, after, ("configmap", "secret"), client.context)
        model.require(model.reference_state(after[1]), "reference_readback_failed")
        expected_spec = json.loads(json.dumps(dep["spec"]))
        i, old_container = model.container(dep)
        expected_spec["template"]["spec"]["containers"][i]["env"] = (
            old_container["env"] if patch is None else old_container["env"] + model.references())
        model.require(after[1]["metadata"]["uid"] == dep["metadata"]["uid"]
                      and after[1]["metadata"]["generation"] == dep["metadata"]["generation"] + (patch is not None)
                      and after[1]["spec"] == expected_spec, "deployment_readback_failed")
        model.same_values(after[0], after[2])
    elif phase == "cleanup":
        patch = model.cleanup_plan(cm, dep, secret, receipt)
        if patch is not None:
            unchanged(before, client.read(), ("configmap", "deployment", "secret"), client.context)
            client.patch("configmap", model.NAME, patch)
        after = client.read()
        unchanged(before, after, ("deployment", "secret"), client.context)
        expected_data = {k: v for k, v in cm["data"].items() if k not in model.KEYS}
        model.require(after[0]["metadata"]["uid"] == cm["metadata"]["uid"]
                      and after[0]["data"] == expected_data, "configmap_readback_failed")
    else:
        raise model.Refused("invalid_phase")
    return model.projection(after, client.context)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("phase", choices=("capture", "seed", "switch", "cleanup", "verify"))
    parser.add_argument("--context", required=True)
    parser.add_argument("--fence", type=Path)
    parser.add_argument("--receipt", type=Path)
    args = parser.parse_args()
    try:
        expected = json.loads(args.fence.read_text())["state"] if args.fence else None
        receipt = json.loads(args.receipt.read_text()) if args.receipt else None
        model.require(args.phase == "capture" or expected is not None, "fence_required")
        state = execute(Kubectl(args.context), args.phase, expected, receipt)
        print(json.dumps({"phase": args.phase, "state": state}, sort_keys=True))
        return 0
    except model.Refused as error:
        print(str(error), file=sys.stderr)
        return 1
    except KeyboardInterrupt:
        print("interrupted_hold_and_recapture", file=sys.stderr)
        return 130
    except Exception:
        # Never print a JSON parser excerpt, CalledProcessError or captured object.
        print("invalid_input_or_unexpected_failure_hold", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
