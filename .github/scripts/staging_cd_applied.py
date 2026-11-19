"""Consume only the digest-bound matching subset from the reviewed staging reader."""
import hashlib
import os
from pathlib import Path
import stat
import sys

from staging_cd_clock import remaining
from staging_cd_common import require
from staging_cd_compatibility import GROUP
from staging_cd_process import run as run_process
from staging_cd_receipts import canonical
from staging_cd_set import document


def configured_group(controller, directory, receipts, started, child_env, *, runner=run_process):
    reader = controller.with_name("applied_contract.py")
    require(reader.is_file() and not reader.is_symlink(), "Reviewed applied-state reader is unavailable")
    candidates = {key: receipts[key]["repository"] + "@" + receipts[key]["digest"] for key in GROUP}
    data = canonical(candidates).encode()
    path = Path(directory) / "contract-candidates.json"
    with path.open("xb") as stream:
        os.chmod(path, 0o600)
        stream.write(data)
    code = runner([sys.executable, "-B", str(reader), "--candidates", str(path)], child_env,
                  min(30, remaining(started)), cwd=controller.parents[2], grace_seconds=6)
    require(code == 0, "Applied contract state could not be verified")
    result = path.with_name("applied-contract.json")
    details = result.lstat()
    require(stat.S_ISREG(details.st_mode) and not details.st_mode & 0o077 and details.st_size <= 512,
            "Configured contract result is invalid")
    value = document(result.read_text(encoding="utf-8"))
    require(isinstance(value, dict) and set(value) == {"schema", "request", "configured"}
            and value["schema"] == "cma-configured-contract-v1"
            and value["request"] == hashlib.sha256(data).hexdigest(), "Configured contract result is unbound")
    configured = value["configured"]
    require(isinstance(configured, list) and all(isinstance(key, str) and key in GROUP for key in configured)
            and configured == sorted(set(configured)), "Configured contract result is invalid")
    return set(configured)
