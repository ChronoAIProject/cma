"""Only fixed, request-bound component outcomes cross the private boundary."""
import os
from pathlib import Path
import stat
import sys

import ci_outcome
from cd_target import name as target_name
from staging_cd_common import matches, require
from staging_cd_receipts import canonical, sha256
from staging_cd_set import document, private_document, release_set, validate_set
from staging_cd_targets import RUNTIME_TARGETS
from staging_cd_versions import DEPLOYABLES

SCHEMA = "cma-private-deployment-result-v1"
STATES = {"controller-verified": "none", "withheld": "deployed-compatibility-unqualified",
          "unverified": "deployment-unverified"}
INVALID_EVIDENCE = "invalid-public-evidence"
POINTER_SCHEMA = "cma-runtime-pointer-promotion-v2"
DIGEST = r"sha256:[0-9a-f]{64}"


def pointer_verified(component, row, environment, *, transition=False):
    if component not in RUNTIME_TARGETS or not isinstance(row, dict):
        return False
    image = row.get("image")
    repository = "ghcr.io/chronoaiproject/" + DEPLOYABLES[component]
    proof = row.get("pointer")
    if not (row.get("state") == "controller-verified" and isinstance(image, str)
            and image.startswith(repository + "@") and matches(DIGEST, image.split("@", 1)[1])
            and matches(r"[0-9a-f]{40}", row.get("source_sha")) and isinstance(proof, dict)
            and set(proof) == {"schema", "environment", "revision", "build", "build_digest", "previous"}
            and proof["schema"] == POINTER_SCHEMA and proof["environment"] == environment
            and type(proof["revision"]) is int and proof["revision"] > 0
            and matches(r"[0-9a-f]{40}", proof["build"]) and matches(DIGEST, proof["build_digest"])):
        return False
    previous = proof["previous"]
    if not (isinstance(previous, dict) and set(previous) == {"digest", "revision"}
            and matches(DIGEST, previous["digest"]) and type(previous["revision"]) is int
            and previous["revision"] > 0):
        return False
    if previous["digest"] == image.split("@", 1)[1]:
        return not transition and previous["revision"] == proof["revision"]
    return previous["revision"] < proof["revision"]


def validate(result, request, code):
    require(isinstance(result, dict) and set(result) == {"schema", "request", "code", "components"}
            and result["schema"] == SCHEMA and result["request"] == sha256(request)
            and type(result["code"]) is int and result["code"] == code and code in (0, 10, 20, 30, 40, 50),
            "Private deployment outcome is unbound")
    rows = result["components"]
    require(isinstance(rows, dict) and set(rows) == set(request["components"]),
            "Private deployment outcome coverage differs")
    for component, row in rows.items():
        required = {"state", "reason", "image", "source_sha"}
        require(isinstance(row, dict) and required <= row.keys() <= required | {"pointer"}
                and isinstance(row["state"], str) and row["state"] in STATES
                and row["reason"] == STATES[row["state"]]
                and row["image"] == request["components"][component]["image"]
                and row["source_sha"] == request["components"][component]["source_sha"],
                "Private deployment outcome contains unexpected data")
        if "pointer" in row:
            require(component in RUNTIME_TARGETS and any(pointer_verified(component, row, environment)
                    for environment in ("staging", "production"))
                    and (component == "runtime" or row["pointer"]["environment"] == "staging"),
                    "Private runtime pointer outcome is invalid")
    states = {row["state"] for row in rows.values()}
    require((code != 0 or states == {"controller-verified"})
            and (code != 50 or states == {"controller-verified", "withheld"})
            and (code in (0, 50) or "controller-verified" not in states),
            "Private deployment outcome contradicts the exit code")
    return result


def read(path, request, code):
    if not path.exists():
        require(code != 50 and (code != 0 or request["schema"] == 1),
                "Private controller has not proved component outcomes")
        return None
    details = path.lstat()
    require(stat.S_ISREG(details.st_mode) and not details.st_mode & 0o077
            and details.st_size <= 16384, "Private deployment outcome file is unsafe")
    return validate(document(path.read_text()), request, code)


def retain(result, env):
    destination = Path(env["RUNNER_TEMP"]) / "public-deployment"
    destination.mkdir(mode=0o700, exist_ok=True)
    output = {"schema": "cma-component-deployment-v1", "run_id": int(env["GITHUB_RUN_ID"]),
              "run_attempt": int(env["GITHUB_RUN_ATTEMPT"]),
              **{key: result[key] for key in ("request", "code", "components")}}
    with (destination / "deployment.json").open("x") as stream:
        os.chmod(stream.name, 0o600)
        stream.write(canonical(output) + "\n")
    return output


def emit(result, request, code, env):
    payload = canonical(validate(result, request, code))
    require(len(payload.encode()) <= 16384, "Component output is oversized")
    # This is a closed projection of public input identities, not private output.
    # It crosses the job boundary only after the same strict result validation.
    with Path(env["GITHUB_OUTPUT"]).open("a") as stream:
        stream.write("component_result=" + payload + "\n")
        verified = {key: request["components"][key] for key, row in result["components"].items()
                    if row["state"] == "controller-verified"}
        if verified:
            from staging_cd_promotion import proof
            stream.write("controller_proof=" + proof(verified) + "\n")


def report(env):
    payload = env.get("CD_COMPONENT_RESULT")
    if not payload:
        return None
    require(len(payload.encode()) <= 16384, "Component output is oversized")
    result = document(payload)
    admitted = validate_set(document(env.get("CD_RELEASE_SET")))["receipts"]
    rows = result.get("components") if isinstance(result, dict) else None
    require(isinstance(rows, dict) and bool(rows) and rows.keys() <= admitted.keys(),
            "Component output differs from admitted selection")
    # Disabled members can be omitted, but the retained subset must reconstruct
    # the exact original private request hash and every public identity.
    request = private_document(release_set({key: admitted[key] for key in rows}), target=target_name(env))
    validate(result, request, result.get("code"))
    return retain(result, env)


def main():
    try:
        report(os.environ)
    except (OSError, ValueError, TypeError, KeyError) as error:
        ci_outcome.report("component-result", INVALID_EVIDENCE, subject="component-result",
                          site=ci_outcome.error_site(error))
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
