"""Only fixed, request-bound component outcomes cross the private boundary."""
import os
from pathlib import Path
import stat
import sys

import ci_outcome
from staging_cd_common import require
from staging_cd_receipts import canonical, sha256
from staging_cd_set import document, private_document, release_set, validate_set

SCHEMA = "cma-private-deployment-result-v1"
STATES = {"controller-verified": "none", "withheld": "deployed-compatibility-unqualified",
          "unverified": "deployment-unverified"}
INVALID_EVIDENCE = "invalid-public-evidence"


def validate(result, request, code):
    require(isinstance(result, dict) and set(result) == {"schema", "request", "code", "components"}
            and result["schema"] == SCHEMA and result["request"] == sha256(request)
            and type(result["code"]) is int and result["code"] == code and code in (0, 10, 20, 30, 40, 50),
            "Private deployment outcome is unbound")
    rows = result["components"]
    require(isinstance(rows, dict) and set(rows) == set(request["components"]),
            "Private deployment outcome coverage differs")
    for component, row in rows.items():
        require(isinstance(row, dict) and set(row) == {"state", "reason", "image", "source_sha"}
                and isinstance(row["state"], str) and row["state"] in STATES
                and row["reason"] == STATES[row["state"]]
                and row["image"] == request["components"][component]["image"]
                and row["source_sha"] == request["components"][component]["source_sha"],
                "Private deployment outcome contains unexpected data")
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
    request = private_document(release_set({key: admitted[key] for key in rows}))
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
