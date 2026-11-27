"""Notify a configured consumer of complete, immutable image publications."""
import os
from pathlib import Path
import subprocess
import sys

from staging_cd_admission import event_identity
from staging_cd_common import matches, require
from staging_cd_component_admission import groups, transition_anchor
from staging_cd_github import GitHub
from staging_cd_receipts import canonical, sha256, validate_receipt
from staging_cd_registry import Registry
from staging_cd_release_tags import local_tag_reader
from staging_cd_runtime import run_private
from staging_cd_set import automatic_set, document
from staging_cd_targets import TARGETS
from staging_cd_versions import REPOSITORY


def validate(value):
    require(isinstance(value, dict) and set(value) == {
        "schema", "repository", "source", "run_id", "run_attempt", "receipts", "id"
    } and value["schema"] == "cma-publication-signal-v1"
        and value["repository"] == REPOSITORY, "Publication signal shape differs")
    require(matches(r"[0-9a-f]{40}", value["source"])
            and all(type(value[key]) is int and value[key] > 0
                    for key in ("run_id", "run_attempt")), "Publication signal identity differs")
    require(isinstance(value["receipts"], dict) and bool(value["receipts"])
            and value["receipts"].keys() <= TARGETS, "Publication signal has invalid components")
    for key, receipt in value["receipts"].items():
        validate_receipt(receipt)
        require(key == receipt["identity"]["component"], "Publication component differs")
    require(value["id"] == sha256({key: item for key, item in value.items() if key != "id"}),
            "Publication signal digest differs")
    return value


def qualification_publication(pr_number, source, receipts, api):
    require(matches(r"[1-9][0-9]{0,19}", pr_number)
            and matches(r"[0-9a-f]{40}", source)
            and receipts.keys() in ({"eg"}, {"web"}), "Qualification requires one Gateway or Web publication")
    pr = api.request("pulls/" + pr_number)
    require(pr.get("merged") is True and pr.get("merge_commit_sha") == source
            and pr.get("base", {}).get("ref") == "develop"
            and pr.get("base", {}).get("repo", {}).get("full_name") == REPOSITORY,
            "Qualification publication source does not match the selected pull request")


def freeze(matrix, env, api, registry):
    tasks = groups(matrix)
    require(bool(tasks), "Publication signal is empty")
    if qualification := env.get("CD_QUALIFICATION_PR"):
        require(env.get("GITHUB_RUN_ATTEMPT") == "1", "Qualification requires the first publication attempt")
        qualification_publication(qualification, env.get("GITHUB_SHA"), tasks, api)
    for component in tasks:
        suffix = component.upper().replace("-", "_")
        require(env.get("CD_PUBLISH_" + suffix) == "success"
                and env.get("CD_FINALIZED_" + suffix) == "success",
                "A selected component did not publish and finalize successfully")
    transition_anchor(matrix)
    normalized = {"include": [{"component": component, "tasks": canonical(entries)}
                              for component, entries in tasks.items()]}
    receipts = automatic_set(normalized, api, registry)["receipts"]
    require(receipts.keys() == tasks.keys(), "Publication receipts differ from the scheduled components")
    if qualification:
        require(all(row["source"] == env["GITHUB_SHA"] and row["identity"]["run_id"] == int(env["GITHUB_RUN_ID"])
                    and row["identity"]["run_attempt"] == 1 for row in receipts.values()),
                "Qualification requires a new image from the selected publication")
    value = {"schema": "cma-publication-signal-v1", "repository": REPOSITORY,
             "source": env["GITHUB_SHA"], "run_id": int(env["GITHUB_RUN_ID"]),
             "run_attempt": int(env["GITHUB_RUN_ATTEMPT"]), "receipts": receipts}
    return validate(dict(value, id=sha256(value)))


def send(value, env, runner=subprocess.run):
    validate(value)
    destination, token = env.get("PUBLICATION_CONSUMER"), env.get("PUBLICATION_SIGNAL_TOKEN")
    require(matches(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", destination)
            and destination.casefold() != REPOSITORY.casefold()
            and all(part not in (".", "..") for part in destination.split("/")),
            "Publication consumer is not configured")
    require(isinstance(token, str) and token and not any(c.isspace() for c in token),
            "Publication signal credential is unavailable")
    child = {key: env[key] for key in ("PATH", "HOME", "LANG", "LC_ALL") if key in env}
    child.update(GH_TOKEN=token, GH_HOST="github.com", GH_PROMPT_DISABLED="1")
    # The consumer receives a hint, with no deployment authority. It must verify
    # the completed source run, immutable artifact, tags and its own policy.
    result = runner(["gh", "api", "--hostname", "github.com", "--method", "POST",
                     "repos/" + destination + "/dispatches", "--input", "-"],
                    input=canonical({"event_type": "cma-images-published-v1", "client_payload": value}),
                    env=child, text=True, capture_output=True, timeout=30, check=False)
    require(result.returncode == 0, "Publication signal delivery failed")


def main():
    def operation(_masker):
        env = os.environ
        event_identity(env)
        require(env["GITHUB_EVENT_NAME"] == "push", "Publication requires a develop push")
        path = Path(env["RUNNER_TEMP"]) / "publication-signal.json"
        if sys.argv[1:] == ["freeze"]:
            api = GitHub(env.get("GH_TOKEN"), local_tags=local_tag_reader(Path.cwd()))
            value = freeze(document(env["CD_PUBLICATION_MATRIX"]), env, api, Registry())
            with path.open("x") as output:
                output.write(canonical(value) + "\n")
        elif sys.argv[1:] == ["send"]:
            require(path.is_file() and not path.is_symlink() and path.stat().st_size <= 65536,
                    "Publication signal artifact is unavailable")
            value = validate(document(path.read_text()))
            require(value["source"] == env["GITHUB_SHA"]
                    and value["run_id"] == int(env["GITHUB_RUN_ID"])
                    and value["run_attempt"] == int(env["GITHUB_RUN_ATTEMPT"]),
                    "Publication signal does not belong to this run attempt")
            send(value, env)
        else:
            return False
        return True
    return run_private(operation, None, subject="publication-signal", explain=False)


if __name__ == "__main__":
    raise SystemExit(main())
