"""Read the current public component versions in one Git ref advertisement."""
import os
import subprocess

from staging_cd_common import Refusal, matches, require
from staging_cd_compatibility import GROUP
from staging_cd_versions import DEPLOYABLES, REPOSITORY, version_tuple


SUPERSEDED = "Automatic deployment version has been superseded"


class SupersededVersions(Refusal):
    def __init__(self, components):
        super().__init__(SUPERSEDED)
        self.components = frozenset(components)


def eligible_receipts(receipts, check, policy, *, obsolete=(), transition=None):
    """Only positive proof of supersession can narrow a previously admitted set."""
    stale = set(obsolete)
    try:
        check(receipts)
    except SupersededVersions as error:
        stale.update(error.components)
    require(stale <= receipts.keys(), "Superseded component is outside the admitted set")
    if stale & GROUP and (receipts.keys() - stale) & GROUP:
        # A transition among the original members requires all three, even if
        # the member defining the transition has just been superseded.
        contract = policy() if transition is None else None
        if transition or any(contract.transition(receipts[key]["source"]) for key in GROUP & receipts.keys()):
            stale.update(GROUP & receipts.keys())
    selected = {key: receipt for key, receipt in receipts.items() if key not in stale}
    require(bool(selected), SUPERSEDED)
    return selected


def record_withheld(env, original, selected):
    withheld = sorted(original.keys() - selected.keys())
    if withheld and env.get("GITHUB_STEP_SUMMARY"):
        with open(env["GITHUB_STEP_SUMMARY"], "a", encoding="utf-8") as summary:
            summary.write("Withheld after supersession or a required contract transition: " + ", ".join(withheld) + "\n")


def require_current_versions(receipts, *, runner=subprocess.run):
    require_version_refs({key: (r["tag"], r["identity"]["version"]) for key, r in receipts.items()},
                         runner=runner, missing="Final version is absent from component inventory")


def advertise(prefixes, *, runner, peeled=False):
    """One credential-free public ref advertisement of the final-tag prefixes."""
    env = {k: os.environ[k] for k in ("PATH", "LANG", "LC_ALL") if k in os.environ}
    env.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull, GIT_TERMINAL_PROMPT="0")
    try:
        result = runner(["git", "-c", "credential.helper=", "ls-remote", *([] if peeled else ["--refs"]),
                         f"https://github.com/{REPOSITORY}.git", *(p + "*" for p in prefixes)],
                        env=env, text=True, capture_output=True, check=False, timeout=10)
    except (OSError, subprocess.SubprocessError) as error:
        raise Refusal("Component version inventory is unavailable") from error
    require(result.returncode == 0 and len(result.stdout) <= 16 * 1024 * 1024,
            "Component version inventory is unavailable")
    return result.stdout


def later_final_sources(receipts, *, runner=subprocess.run):
    """Each component's commits tagged by finals above its receipt's version.

    A final tag's peeled commit is its publication source, so this lets a
    caller prove a newer final at a later develop source without credentials.
    """
    require(bool(receipts) and receipts.keys() <= DEPLOYABLES.keys(), "Unknown release component")
    prefixes = {f"refs/tags/{DEPLOYABLES[key]}/v": key for key in receipts}
    later = {key: [] for key in receipts}
    for line in advertise(prefixes, runner=runner, peeled=True).splitlines():
        oid, separator, ref = line.partition("\t")
        prefix = next((p for p in prefixes if ref.startswith(p)), None)
        require(separator and matches(r"[0-9a-f]{40}", oid) and prefix, "Component version inventory is malformed")
        if not ref.endswith("^{}"):
            continue
        component = prefixes[prefix]
        if version_tuple(ref[len(prefix):-3]) > version_tuple(receipts[component]["identity"]["version"]):
            later[component].append(oid)
    return later


def require_version_refs(expected, *, runner=subprocess.run, missing="Expected version is absent from component inventory"):
    # Every lane finishes a started build, so a successor's pending reservation
    # never withholds a final. Only a higher final in the same advertisement does.
    require(bool(expected) and expected.keys() <= DEPLOYABLES.keys(), "Unknown release component")
    prefixes = {f"refs/tags/{DEPLOYABLES[key]}/v": key for key in expected}
    stdout = advertise(prefixes, runner=runner)
    found, stale = set(), set()
    for line in stdout.splitlines():
        oid, separator, ref = line.partition("\t")
        prefix = next((p for p in prefixes if ref.startswith(p)), None)
        require(separator and matches(r"[0-9a-f]{40}", oid) and prefix and ref not in found,
                "Component version inventory is malformed")
        found.add(ref)
        version = version_tuple(ref[len(prefix):])
        component = prefixes[prefix]
        if version > version_tuple(expected[component][1]):
            stale.add(component)
    require(all("refs/tags/" + tag in found for tag, _ in expected.values()), missing)
    if stale:
        raise SupersededVersions(stale)
