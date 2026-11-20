"""Atomic version reservations preserve burned versions across concurrent runs."""
import base64
import json
import os
import subprocess
import time

from deployables import known_components

from staging_cd_common import Refusal, matches, require
from staging_cd_receipts import canonical, final_tag_oid, final_tag_text, validate_receipt
from staging_cd_versions import BASE_VERSION, DEPLOYABLES, LEVELS, bump, version_tuple

KNOWN_DEPLOYABLES = known_components()
FIELDS = {"schema", "component", "version", "previous", "source", "tree", "pr", "bump", "run_id", "run_attempt"}


class ReservationSuperseded(Refusal):
    """A validated same-component successor has taken this reservation."""

    def __init__(self):
        super().__init__("Publication reservation is superseded and burned")


def validate_reservation(value):
    require(isinstance(value, dict) and set(value) == FIELDS and value["schema"] == "cma-version-reservation-v1",
            "Version reservation is malformed")
    require(value["component"] in KNOWN_DEPLOYABLES and value["bump"] in LEVELS.values()
            and bump(value["previous"], value["bump"]) == value["version"], "Reservation version is invalid")
    require(all(matches(r"[0-9a-f]{40}", value[key]) for key in ("source", "tree"))
            and all(type(value[key]) is int and value[key] > 0 for key in ("pr", "run_id", "run_attempt")),
            "Reservation source or run identity is invalid")
    return value


def pending_name(value):
    validate_reservation(value)
    return KNOWN_DEPLOYABLES[value["component"]] + "/pending/v" + value["version"]


def outcome_ref(component, version):
    require(component in DEPLOYABLES, "Unknown reservation component")
    version_tuple(version)
    return f"refs/tags/cma-allocation-outcome/{component}/v{version}"


def next_reservation(component, source, tree, pr, level, run_id, attempt, finals, pending):
    require(component in DEPLOYABLES, "Unknown reservation component")
    versions = [BASE_VERSION]
    for tag in finals:
        prefix = DEPLOYABLES[component] + "/v"
        if tag["name"].startswith(prefix):
            versions.append(tag["name"][len(prefix):])
    for item in pending:
        validate_reservation(item)
        if item["component"] == component:
            versions.append(item["version"])
    previous = max(versions, key=version_tuple)
    return validate_reservation({"schema": "cma-version-reservation-v1", "component": component,
                                 "version": bump(previous, level), "previous": previous, "source": source,
                                 "tree": tree, "pr": pr, "bump": level, "run_id": run_id, "run_attempt": attempt})


def reserve(allocate, insert, refresh, *, attempts=5):
    """A lost insert race consumes that version; refresh before another attempt."""
    for _ in range(attempts):
        finals, pending = refresh()
        reservation = allocate(finals, pending)
        if insert(reservation):
            return reservation
    raise Refusal("Version reservation collision limit exceeded")


class Reservations:
    def __init__(self, api, token, root, runner=subprocess.run, *, mark=lambda _stage: None, sleep=time.sleep):
        self.api, self.root, self.runner = api, root, runner
        self.mark, self.sleep = mark, sleep
        require(isinstance(token, str) and token and not any(c.isspace() for c in token),
                "Version reservation credential is invalid")
        header = "AUTHORIZATION: basic " + base64.b64encode(("x-access-token:" + token).encode()).decode()
        self.env = {key: value for key, value in os.environ.items() if key in ("PATH", "HOME", "LANG", "LC_ALL")}
        self.env.update(GIT_TERMINAL_PROMPT="0", GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull,
                        GIT_CONFIG_COUNT="1", GIT_CONFIG_KEY_0="http.https://github.com/.extraheader",
                        GIT_CONFIG_VALUE_0=header)

    def list(self, component=None):
        require(component is None or component in DEPLOYABLES, "Unknown reservation component")
        result = []
        for key, deployable in DEPLOYABLES.items():
            if component is not None and component != key:
                continue
            rows = self.api.request("git/matching-refs/tags/" + deployable + "/pending/v")
            require(isinstance(rows, list), "Pending tag inventory is malformed")
            for row in rows:
                require(isinstance(row, dict) and isinstance(row.get("object"), dict)
                        and row["object"].get("type") == "tag" and matches(r"[0-9a-f]{40}", row["object"].get("sha")),
                        "Pending tag lacks reservation provenance")
                tag = self.api.request("git/tags/" + row["object"]["sha"])
                value = validate_reservation(json.loads(tag["message"]))
                require(value["component"] == key and tag.get("tag") == pending_name(value)
                        and row.get("ref") == "refs/tags/" + pending_name(value)
                        and isinstance(tag.get("object"), dict) and tag["object"].get("type") == "commit"
                        and tag["object"].get("sha") == value["source"],
                        "Pending version ref differs from its reservation")
                result.append(value)
        require(len({pending_name(item) for item in result}) == len(result), "Duplicate pending version")
        return result

    def command(self, *args, data=None):
        return self.runner(["git", *args], cwd=self.root, env=self.env, input=data, text=True,
                           capture_output=True, timeout=60, check=False)

    def insert(self, value):
        validate_reservation(value)
        require(value["component"] in DEPLOYABLES, "Retired images cannot reserve versions")
        # An annotated reservation records the owning run, so a later planner
        # can distinguish an active build from a burned failed run. The push
        # itself is ordinary insert-only git push, never force/lease/update.
        text = (f"object {value['source']}\ntype commit\ntag {pending_name(value)}\n"
                "tagger github-actions[bot] <41898282+github-actions[bot]@users.noreply.github.com> 0 +0000\n\n"
                + canonical(value) + "\n")
        created = self.command("mktag", data=text)
        require(created.returncode == 0 and matches(r"[0-9a-f]{40}", created.stdout.strip()),
                "Reservation object could not be created")
        result = self.command("push", "--porcelain", "https://github.com/ChronoAIProject/cma.git",
                              created.stdout.strip() + ":refs/tags/" + pending_name(value))
        if result.returncode != 0:
            # Only a demonstrated collision permits retry. Authentication or
            # transport failures must not become an unbounded version burner.
            existing = [item for item in self.list(value["component"]) if pending_name(item) == pending_name(value)]
            require(len(existing) == 1, "Reservation push failed without a collision")
            if existing[0] == value:
                # Transport ambiguity is resolved only by the exact unique
                # allocation owner, never by an arbitrary successful ref read.
                return True
            return False
        require(any(line.startswith("*\t") and "[new tag]" in line for line in result.stdout.splitlines()),
                "An up-to-date push is not a new version reservation")
        require(value in self.list(value["component"]), "Reservation push was not verified")
        return True

    def require_current(self, value):
        """Supersession is derived from immutable tags; burned refs stay intact."""
        validate_reservation(value)
        component = value["component"]
        pending, finals = self.list(component), self.api.tags(component)
        require(value in pending, "Publication reservation is missing")
        history = self.api.history()
        position = history.position(value["source"])
        candidates = pending + [tag["receipt"]["identity"] for tag in finals]
        if component == "am":
            for item in candidates:
                if version_tuple(item["version"]) > version_tuple(value["version"]):
                    # A larger number alone still fences writes, but cannot
                    # turn malformed or unrelated provenance into a handled skip.
                    chain = history
                    if item["source"] not in chain.positions:
                        chain = self.api.history_containing(item["source"])
                    if chain.position(item["source"]) >= chain.position(value["source"]):
                        raise ReservationSuperseded()
        require(all((item["source"] not in history.positions or history.position(item["source"]) <= position)
                    and version_tuple(item["version"]) <= version_tuple(value["version"])
                    for item in candidates), "Publication reservation is superseded and burned")
        return True

    def remove_after_publication(self, receipt, expected_oid):
        self.mark("cleanup_verify")
        component = receipt["identity"]["component"]
        require(component in DEPLOYABLES, "Retired image tags must remain unchanged")
        finals = [tag for tag in self.api.tags(component) if tag["name"] == receipt["tag"]]
        require(len(finals) == 1 and finals[0]["receipt"] == receipt and finals[0]["oid"] == expected_oid,
                "Pending deletion requires the verified matching final receipt")
        self.api.require_current_versions({component: receipt})
        matches = [item for item in self.list(component) if item["version"] == receipt["identity"]["version"]]
        if not matches:
            return
        require(len(matches) == 1 and all(matches[0][key] == receipt["identity"][key]
                                         for key in ("source", "run_id", "run_attempt", "version", "pr")),
                "Pending version belongs to another source")
        self.mark("cleanup_delete")
        self.command("push", "https://github.com/ChronoAIProject/cma.git",
                     ":refs/tags/" + pending_name(matches[0]))
        self.mark("cleanup_read")
        remaining = [item for item in self.list(component) if item["version"] == receipt["identity"]["version"]]
        require(not remaining, "Published reservation cleanup failed")

    def finalize(self, receipt, verify_digest, *, recovery_oid=None):
        self.mark("receipt")
        validate_receipt(receipt)
        component = receipt["identity"]["component"]
        require(component in DEPLOYABLES, "Retired image tags must remain unchanged")
        self.mark("registry_verify")
        require(verify_digest(receipt["repository"], receipt["digest"]) is True,
                "Final version requires the verified published digest")
        self.mark("final_read")
        existing = [tag for tag in self.api.tags(component) if tag["target"] == receipt["source"]]
        if recovery_oid is not None:
            # A prior attempt may only settle the exact final observed at
            # batch preflight. Disappearance must never fall through to a push.
            require(len(existing) == 1 and existing[0].get("oid") == recovery_oid,
                    "Prior-attempt final tag object differs")
        if existing:
            expected_oid = final_tag_oid(receipt)
            require(len(existing) == 1 and existing[0]["receipt"] == receipt
                    and existing[0].get("oid") == expected_oid,
                    "This merge already has a different immutable release")
            self.remove_after_publication(receipt, expected_oid)
            return False
        expected = {key: receipt["identity"][key] for key in FIELDS - {"schema"}}
        expected["schema"] = "cma-version-reservation-v1"
        self.mark("reservation_read")
        self.require_current(expected)
        self.mark("tag_create")
        created = self.command("mktag", data=final_tag_text(receipt))
        require(created.returncode == 0 and matches(r"[0-9a-f]{40}", created.stdout.strip()),
                "Final annotation could not be created")
        expected_oid = created.stdout.strip()
        self.mark("tag_push")
        # A successor and a finalizer compete for the same insert-only outcome.
        # Even takeover during the push cannot produce an obsolete final tag.
        result = self.command("push", "--atomic", "--porcelain", "https://github.com/ChronoAIProject/cma.git",
                              expected_oid + ":" + outcome_ref(component, expected["version"]),
                              expected_oid + ":refs/tags/" + receipt["tag"])
        # A lost response is not absence: settle the remote final before retry.
        self.mark("final_verify")
        self.verify_final(receipt, expected_oid)
        require(result.returncode != 0 or any(line.startswith("*\t") and "[new tag]" in line
                                             for line in result.stdout.splitlines()),
                "Final publication unexpectedly reported an up-to-date ref")
        self.remove_after_publication(receipt, expected_oid)
        return True

    def verify_final(self, receipt, expected_oid):
        # An insert may precede its inventory readback. Retry only empty reads;
        # never repeat the write, suppress an API error or wait out a conflict.
        for attempt in range(3):
            verified = [tag for tag in self.api.tags(receipt["identity"]["component"])
                        if tag["target"] == receipt["source"] or tag["name"] == receipt["tag"]]
            if verified:
                require(len(verified) == 1 and verified[0]["receipt"] == receipt
                        and verified[0]["oid"] == expected_oid,
                        "Final tag conflicts with the published receipt")
                return
            if attempt < 2:
                self.sleep(1)
        require(False, "Final tag creation could not be verified")
