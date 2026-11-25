"""Read the serving Web preimage through the pinned read-only staging controller."""

from pathlib import Path
import sys
import tempfile

from cd_target import STAGING, name as target_name
from staging_cd_admission import event_identity
from staging_cd_clock import TERMINATION_GRACE_SECONDS, remaining
from staging_cd_common import require
from staging_cd_invoke import IDENTITY_KEYS, PROCESS_KEYS, private_script
from staging_cd_process import run as run_process
from staging_cd_set import document
from staging_cd_tools import environment as tool_environment


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
