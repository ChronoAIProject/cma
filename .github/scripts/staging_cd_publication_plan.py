"""Qualify one staging publication from the existing CMA deployment runner."""
import base64
import json
import os
from pathlib import Path
import stat
import sys

from cd_target import STAGING, name as target_name
from staging_cd_admission import event_identity, maintainer_dispatch
from staging_cd_app_metadata import REPOSITORY as PRIVATE_REPOSITORY, github_reader
from staging_cd_common import matches, require
from staging_cd_github import GitHub
from staging_cd_invoke import private_script
from staging_cd_publication_signal import qualification_publication
from staging_cd_runtime import run_private
from staging_cd_tools import environment as tool_environment

POLICY_FILE = "staging/publication-policy.json"
INPUT_FILE = "publication-plan-input.json"
REPORT_FILE = "publication-plan.json"


def selector(env):
    event = event_identity(env)
    require(target_name(env) == STAGING and env["GITHUB_EVENT_NAME"] == "workflow_dispatch",
            "Publication plan requires a staging dispatch")
    inputs = event["inputs"]
    require(inputs.get("mode") == "publication-plan", "Publication plan mode is invalid")
    fields = ("publication_run", "publication_attempt", "publication_signal")
    run, attempt, signal = (inputs.get(key) for key in fields)
    require(matches(r"[1-9][0-9]{0,19}", run) and attempt == "1"
            and matches(r"[0-9a-f]{64}", signal), "Publication plan selector is invalid")
    return int(run), int(attempt), signal


def private_policy(env, read=github_reader):
    ref = env.get("CD_PUBLICATION_POLICY_REF")
    require(matches(r"[0-9a-f]{40}", ref), "Publication plan policy must use a pinned revision")
    value = read(env.get("CD_INFRA_TOKEN"))(f"repos/{PRIVATE_REPOSITORY}/contents/{POLICY_FILE}?ref={ref}")
    require(value.get("type") == "file" and value.get("encoding") == "base64"
            and isinstance(value.get("content"), str) and len(value["content"]) <= 4096,
            "Publication plan policy is unavailable")
    return base64.b64decode("".join(value["content"].splitlines()), validate=True)


def write(path, value):
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "w") as output:
        json.dump(value, output, sort_keys=True, separators=(",", ":"))


def prepare(env, target, private, root):
    from publication_api import PublicAPI
    from publication_contract import read_policy
    from publication_verify import verified_signal

    policy_path = root / "publication-plan-policy.json"
    fd = os.open(policy_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "wb") as stream:
        stream.write(private_policy(env))
    token = env.get("GH_TOKEN")
    api = PublicAPI(token)
    maintainer_dispatch(env, GitHub(token).request)
    signal = verified_signal(api, *target, read_policy(policy_path))
    qualification_publication(env.get("CD_QUALIFICATION_PR"), signal["source"], signal["receipts"], api)
    require(all(row["source"] == signal["source"] and row["identity"]["run_id"] == target[0]
                and row["identity"]["run_attempt"] == target[1] for row in signal["receipts"].values()),
            "Qualification requires a new image from the selected publication")
    private.releases(signal, private.load_config())
    write(root / INPUT_FILE, signal)


def execute(env, target, private, root):
    from publication_contract import document, read_policy, validate

    policy = read_policy(root / "publication-plan-policy.json")
    signal_path = root / INPUT_FILE
    metadata = signal_path.lstat()
    require(stat.S_ISREG(metadata.st_mode) and metadata.st_mode & 0o077 == 0
            and 0 < metadata.st_size <= 65536, "Prepared publication plan is unavailable")
    signal = validate(document(signal_path.read_bytes()), policy)
    require((signal["run_id"], signal["run_attempt"], signal["id"]) == target,
            "Prepared publication plan identity differs")
    require(signal["receipts"].keys() in ({"eg"}, {"web"}), "Qualification requires one Gateway or Web publication")
    config = private.load_config()
    [batch] = private.releases(signal, config)
    tools = tool_environment(env)
    os.environ.update(tools)
    started = private.timestamp()
    client = private.StagingReadOnly(private.StagingChronoCD())
    options = {}
    if "asset_continuity" in batch.get("web", {}):
        options["engine_factory"] = private.WebContinuityEngine
    runner = private.Coordinator(
        client, config,
        acceptance=private.Acceptance(config, identity=private.BuildIdentity(config, client=client)),
        namespace_guard=private.NamespaceGuard(client), **options,
    )
    require(runner.execute(batch, "release", started, read_only=True) == 0,
            "Staging publication plan refused")
    write(root / REPORT_FILE, {
        "schema": 1, "run_id": target[0], "run_attempt": target[1], "signal_id": target[2],
        "source": signal["source"], "resource_methods": ["GET"], "mutations": 0,
        "components": private.plan_comparison(batch, runner.states),
    })


def main():
    def operation(masker):
        env = os.environ
        target = selector(env)
        scripts = private_script(env).parent
        require(not (scripts / "staging_publication_promotion.py").is_symlink(),
                "Private plan entry point links are forbidden")
        sys.path.append(str(scripts))
        import staging_publication_promotion as private

        root = Path(env["RUNNER_TEMP"])
        require(root.is_absolute() and root.is_dir() and not root.is_symlink(),
                "Runner temporary directory is invalid")
        if sys.argv[1:] == ["prepare"]:
            masker(env.get("CD_INFRA_TOKEN"))
            prepare(env, target, private, root)
        elif sys.argv[1:] == ["execute"]:
            masker([env.get("CD_NYXID_CLIENT_ID"), env.get("CD_NYXID_CLIENT_SECRET")])
            execute(env, target, private, root)
        else:
            return False
        return True
    return run_private(operation, None, subject="publication-plan", explain=False)


if __name__ == "__main__":
    raise SystemExit(main())
