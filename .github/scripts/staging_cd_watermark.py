"""Read the current public component versions in one Git ref advertisement."""
import os
import subprocess

from staging_cd_common import Refusal, matches, require
from staging_cd_versions import DEPLOYABLES, REPOSITORY, version_tuple


SUPERSEDED = "Automatic deployment version has been superseded"


def require_current_versions(receipts, *, runner=subprocess.run):
    # A final tag can predate a newer pending reservation. Reading both from
    # one advertisement also covers a finalizer concurrently removing pending.
    require(bool(receipts) and receipts.keys() <= DEPLOYABLES.keys(), "Unknown release component")
    prefixes = {f"refs/tags/{DEPLOYABLES[key]}/{suffix}": key
                for key in receipts for suffix in ("v", "pending/v")}
    env = {k: os.environ[k] for k in ("PATH", "LANG", "LC_ALL") if k in os.environ}
    env.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull, GIT_TERMINAL_PROMPT="0")
    try:
        result = runner(["git", "-c", "credential.helper=", "ls-remote", "--refs",
                         f"https://github.com/{REPOSITORY}.git", *(p + "*" for p in prefixes)],
                        env=env, text=True, capture_output=True, check=False, timeout=10)
    except (OSError, subprocess.SubprocessError) as error:
        raise Refusal("Component version inventory is unavailable") from error
    require(result.returncode == 0 and len(result.stdout) <= 16 * 1024 * 1024,
            "Component version inventory is unavailable")
    found = set()
    for line in result.stdout.splitlines():
        oid, separator, ref = line.partition("\t")
        prefix = next((p for p in prefixes if ref.startswith(p)), None)
        require(separator and matches(r"[0-9a-f]{40}", oid) and prefix and ref not in found,
                "Component version inventory is malformed")
        found.add(ref)
        version = version_tuple(ref[len(prefix):])
        require(version <= version_tuple(receipts[prefixes[prefix]]["identity"]["version"]), SUPERSEDED)
    require(all("refs/tags/" + receipt["tag"] in found for receipt in receipts.values()),
            "Final version is absent from component inventory")
