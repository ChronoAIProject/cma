"""Read the serving Web preimage through the pinned read-only staging controller."""

from pathlib import Path
import os
import sys
import tempfile

from cd_target import STAGING, name as target_name
from staging_cd_admission import event_identity
from staging_cd_clock import TERMINATION_GRACE_SECONDS, remaining, timestamp
from staging_cd_common import require
from staging_cd_github import GitHub
from staging_cd_invoke import IDENTITY_KEYS, PROCESS_KEYS, private_script
from staging_cd_process import run as run_process
from staging_cd_receipts import canonical
from staging_cd_release_tags import local_tag_reader
from staging_cd_resolve import output
from staging_cd_runtime import run_private
from staging_cd_set import document
from staging_cd_tools import environment as tool_environment
from staging_cd_web_continuity import DEFAULT_RETENTION_ROLLOUTS, retention_scope


def read(env, masker, *, script_reader=private_script, runner=run_process, tool_reader=tool_environment):
    event_identity(env)
    require(env["GITHUB_EVENT_NAME"] == "push" and target_name(env) == STAGING,
            "Web publish preimage requires a staging develop push")
    started = env.get("CD_STARTED_AT")
    remaining(started, mode="release")
    script = script_reader(env)
    credentials = {key: env.get(key) for key in ("CD_NYXID_CLIENT_ID", "CD_NYXID_CLIENT_SECRET")}
    for credential in credentials.values():
        masker(credential)
    require(all(isinstance(value, str) and value and not any(c.isspace() for c in value)
                for value in credentials.values()), "Web preimage credentials are missing or malformed")
    child_env = {key: env[key] for key in (*IDENTITY_KEYS, *PROCESS_KEYS) if key in env}
    child_env.update(tool_reader(env))
    child_env.update(credentials)
    with tempfile.TemporaryDirectory(prefix="cma-web-preimage-", dir=env["RUNNER_TEMP"]) as directory:
        path = Path(directory) / "preimage.json"
        args = [sys.executable, "-B", str(script), "--mode", "release", "--preflight-only",
                "--web-preimage", str(path), "--started-at", started]
        budget = remaining(started, mode="release")
        require(runner(args, child_env, budget, cwd=script.parents[2], grace_seconds=TERMINATION_GRACE_SECONDS) == 0,
                "Web controller preimage was not established")
        require(not path.is_symlink() and path.is_file(), "Web controller preimage file is missing")
        info = path.stat()
        require((info.st_mode & 0o777) == 0o600 and 0 < info.st_size <= 4096,
                "Web controller preimage file is not private or is oversized")
        return document(path.read_text(encoding="utf-8"))


def seed_document(path):
    require(not path.is_symlink(), "Web asset bootstrap links are forbidden")
    if not path.is_file():
        return None
    require(0 < path.stat().st_size <= 65536, "Web asset bootstrap document is oversized")
    return document(path.read_text(encoding="utf-8"))


def main():
    def operation(masker):
        env = dict(os.environ)
        script = private_script(env)
        env["CD_STARTED_AT"] = timestamp()
        preimage = read(env, masker, script_reader=lambda _: script)
        # The only legacy adoption inputs come from the same immutable
        # infrastructure checkout as the controller.
        bootstrap, evidence = None, None
        if preimage.get("asset_manifest_sha256") is None:
            root = script.parents[1]
            bootstrap = seed_document(root / "web-asset-bootstrap.json")
            evidence = seed_document(root / "web-asset-bootstrap-evidence.json")
        api = GitHub(env.get("GH_TOKEN"), local_tags=local_tag_reader(Path.cwd()))
        tags = api.tags("web")
        history = api.history()
        for tag in tags:
            history.position(tag["target"])
        retain_rollouts = env.get("CD_WEB_RETENTION_ROLLOUTS")
        scope = retention_scope(preimage, env["CD_STARTED_AT"], [tag["receipt"] for tag in tags],
                                bootstrap=bootstrap, bootstrap_evidence=evidence,
                                retain_rollouts=int(retain_rollouts) if retain_rollouts else DEFAULT_RETENTION_ROLLOUTS)
        output(env, "scope", canonical(scope))
        return True

    return run_private(operation, None, subject="web-preimage")


if __name__ == "__main__":
    raise SystemExit(main())
