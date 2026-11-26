"""Atomic version reservations preserve burned versions across concurrent runs."""
import base64
import json
import os
import subprocess
import time

from deployables import known_components

from staging_cd_common import Refusal, matches, require
from staging_cd_receipts import canonical, final_tag_oid, final_tag_text, validate_receipt
from staging_cd_versions import (
    DEPLOYABLES, LEVELS, bump, require_publishable_release, require_publishable_source,
    source_is_retired, version_floor, version_tuple,
)

KNOWN_DEPLOYABLES = known_components()
FIELDS = {"schema", "component", "version", "previous", "source", "tree", "pr", "bump", "run_id", "run_attempt"}
# A pending tag and its final belong to one owner exactly when these agree.
OWNER = ("source", "run_id", "run_attempt", "version", "pr")
# GitHub acknowledges a ref push before its REST ref listing shows it, so a
# readback right after the push can still see the old state for seconds. Re-read
# only that lag, after each delay in turn; never repeat the write or hide an
# API error.
READBACK_SETTLE_SECONDS = (1, 2, 4, 8)
# One finalization prunes at most this many burned reservations; the next prunes the rest.
PRUNE_LIMIT = 128
# No pruning Git call starts with less of the finalization budget than this.
PRUNE_STEP_SECONDS = 5
REMOTE = "https://github.com/ChronoAIProject/cma.git"


class ReservationSuperseded(Refusal):
    """A validated same-component successor, or a final of the same source, has taken this reservation."""

    def __init__(self):
        super().__init__("Publication reservation is superseded and burned")


class SourceAlreadyFinal(ReservationSuperseded):
    """An older run finalized this exact source first, so no newer run owns its deployment."""


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


def claim_ref(value):
    # Bumps from one predecessor can have different version names (minor vs
    # patch). Both must compete for the same immutable successor slot.
    return "refs/tags/cma-allocation/" + value["component"] + "/after-v" + value["previous"]


def outcome_ref(component, version):
    require(component in DEPLOYABLES, "Unknown reservation component")
    version_tuple(version)
    return f"refs/tags/cma-allocation-outcome/{component}/v{version}"


def next_reservation(component, source, tree, pr, level, run_id, attempt, finals, pending):
    require(component in DEPLOYABLES, "Unknown reservation component")
    require_publishable_source(component, source)
    versions = [version_floor(component)]
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
        # The complete final inventory each cleanup verified, by component.
        self.settled = {}
        require(isinstance(token, str) and token and not any(c.isspace() for c in token),
                "Version reservation credential is invalid")
        header = "AUTHORIZATION: basic " + base64.b64encode(("x-access-token:" + token).encode()).decode()
        self.env = {key: value for key, value in os.environ.items() if key in ("PATH", "HOME", "LANG", "LC_ALL")}
        self.env.update(GIT_TERMINAL_PROMPT="0", GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull,
                        GIT_CONFIG_COUNT="1", GIT_CONFIG_KEY_0="http.https://github.com/.extraheader",
                        GIT_CONFIG_VALUE_0=header)

    def list(self, component=None):
        require(component is None or component in DEPLOYABLES, "Unknown reservation component")
        result = [value for key in DEPLOYABLES if component in (None, key) for value, _ in self.inventory(key)]
        require(len({pending_name(item) for item in result}) == len(result), "Duplicate pending version")
        return result

    def settled_list(self, component, done):
        """The lane's reservations once done(rows) holds, else the last read of a bounded settle."""
        rows = self.list(component)
        for delay in READBACK_SETTLE_SECONDS:
            if done(rows):
                break
            self.sleep(delay)
            rows = self.list(component)
        return rows

    def inventory(self, key, rows=None):
        """One lane's validated reservations, each with the tag object its live ref names."""
        if rows is None:
            rows = self.api.request("git/matching-refs/tags/" + DEPLOYABLES[key] + "/pending/v")
        require(isinstance(rows, list), "Pending tag inventory is malformed")
        for row in rows:
            require(isinstance(row, dict) and isinstance(row.get("object"), dict)
                    and row["object"].get("type") == "tag" and matches(r"[0-9a-f]{40}", row["object"].get("sha")),
                    "Pending tag lacks reservation provenance")
        self.api.preload_tags(rows)
        result = []
        for row in rows:
            tag = self.api.request("git/tags/" + row["object"]["sha"])
            value = validate_reservation(json.loads(tag["message"]))
            require(value["component"] == key and tag.get("tag") == pending_name(value)
                    and row.get("ref") == "refs/tags/" + pending_name(value)
                    and isinstance(tag.get("object"), dict) and tag["object"].get("type") == "commit"
                    and tag["object"].get("sha") == value["source"],
                    "Pending version ref differs from its reservation")
            result.append((value, row["object"]["sha"]))
        return result

    def advertised(self, component, *, timeout=60):
        """One lane's pending tags and allocation successor claims, from one live advertisement.

        Allocation pushes a reservation and its claim atomically, so a single
        advertisement shows both or neither. Separate listings could miss a
        claim and leave it without the reservation that backs it.
        """
        prefixes = ("refs/tags/" + DEPLOYABLES[component] + "/pending/v",
                    "refs/tags/cma-allocation/" + component + "/after-v")
        result = self.command("ls-remote", "--refs", REMOTE, *(prefix + "*" for prefix in prefixes), timeout=timeout)
        require(result.returncode == 0 and len(result.stdout) <= 16 * 1024 * 1024,
                "Burned reservation inventory is unavailable")
        pending, claims = {}, {}
        for line in result.stdout.splitlines():
            oid, separator, ref = line.partition("\t")
            require(separator and matches(r"[0-9a-f]{40}", oid) and ref.startswith(prefixes)
                    and ref not in pending and ref not in claims, "Burned reservation inventory is malformed")
            (pending if ref.startswith(prefixes[0]) else claims)[ref] = oid
        return pending, claims

    def command(self, *args, data=None, timeout=60):
        return self.runner(["git", *args], cwd=self.root, env=self.env, input=data, text=True,
                           capture_output=True, timeout=timeout, check=False)

    def insert(self, value):
        validate_reservation(value)
        require(value["component"] in DEPLOYABLES, "Retired images cannot reserve versions")
        require_publishable_release(value)
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
        rows = self.settled_list(value["component"], lambda rows: value in rows)
        require(value in rows, "Reservation push was not verified")
        return True

    def require_current(self, value, *, build_started):
        """Supersession is derived from immutable tags; burned refs stay until a final fences them.

        Every lane queues only its newest reservation and finishes a build it
        has started. Each caller states whether this reservation's build has
        started: before it, a newer reservation or final, or a final of the
        same source, takes over and this one yields; after it, only a final
        fences it, and that is a failure.
        """
        validate_reservation(value)
        component = value["component"]
        require_publishable_release(value)
        pending, finals = self.list(component), self.api.tags(component)
        if value not in pending:
            # Pruning removes a reservation only once a final at or after its
            # source has fenced it. A queued run still yields to that final
            # before building; a started build or any other absence fails.
            require(not build_started, "Publication reservation is missing")
            history = self.api.history()
            require(any(self.supersedes(tag["receipt"]["identity"], value, history) for tag in finals),
                    "Publication reservation is missing")
            raise ReservationSuperseded()
        history = self.api.history()
        position = history.position(value["source"])
        if build_started:
            # The successor waits for this build instead of revoking it. A
            # higher or later final still fails below; it never becomes a skip.
            pending = [item for item in pending if version_tuple(item["version"]) <= version_tuple(value["version"])]
        candidates = pending + [tag["receipt"]["identity"] for tag in finals]
        if not build_started:
            for item in candidates:
                if self.supersedes(item, value, history):
                    raise ReservationSuperseded()
        require(all((item["source"] not in history.positions or history.position(item["source"]) <= position)
                    and version_tuple(item["version"]) <= version_tuple(value["version"])
                    for item in candidates), "Publication reservation is superseded and burned")
        # A source has one final per component. When an earlier same-source
        # build finishes first, as a contract group's fresh claim allows, this
        # reservation can never finalize: the source is already published, so
        # it yields before building. After its build starts it still fails.
        if any(tag["receipt"]["source"] == value["source"] for tag in finals):
            if not build_started:
                raise SourceAlreadyFinal()
            raise Refusal("This merge already has a different immutable release")
        return True

    def supersedes(self, item, value, history):
        """A higher version from the same or a later develop source takes over a queued reservation."""
        if version_tuple(item["version"]) <= version_tuple(value["version"]):
            return False
        # A larger number alone still fences writes, but cannot turn
        # malformed or unrelated provenance into a handled skip.
        chain = history
        if item["source"] not in chain.positions:
            chain = self.api.history_containing(item["source"])
        return chain.position(item["source"]) >= chain.position(value["source"])

    def remove_after_publication(self, receipt, expected_oid):
        self.mark("cleanup_verify")
        component = receipt["identity"]["component"]
        require(component in DEPLOYABLES, "Retired image tags must remain unchanged")
        require_publishable_release(receipt["identity"])
        observed = self.api.tags(component)
        finals = [tag for tag in observed if tag["name"] == receipt["tag"]]
        require(len(finals) == 1 and finals[0]["receipt"] == receipt and finals[0]["oid"] == expected_oid,
                "Pending deletion requires the verified matching final receipt")
        self.api.require_current_versions({component: receipt})
        self.settled[component] = observed
        matches = [item for item in self.list(component) if item["version"] == receipt["identity"]["version"]]
        if not matches:
            return
        require(len(matches) == 1 and all(matches[0][key] == receipt["identity"][key] for key in OWNER),
                "Pending version belongs to another source")
        self.mark("cleanup_delete")
        self.command("push", REMOTE, ":refs/tags/" + pending_name(matches[0]))
        self.mark("cleanup_read")
        version = receipt["identity"]["version"]
        rows = self.settled_list(component, lambda rows: all(item["version"] != version for item in rows))
        require(all(item["version"] != version for item in rows), "Published reservation cleanup failed")

    def prune_burned(self, receipt, *, deadline=None, clock=time.monotonic):
        """Remove this lane's reservations that its verified finals have made unusable.

        Runs after finalization verified the lane's new final and its own
        cleanup. A lane runs one release at a time, so no other build of it is
        running; a burned attempt can never finalize, and finals are never
        removed. A reservation below a final from the same or a later source
        therefore can never publish: require_current refuses it once its build
        starts. A queued run still holding one yields to that final before
        building, exactly as with the tag present. A reservation whose own
        final exists is settled. The current run's reservations, burned
        versions above every final, and any whose claim names another object
        remain. Each removed tag takes the cma-allocation/ successor claim only
        it backs, both read in one advertisement and deleted in one atomic push
        leased to the observed objects, so every claim left stays backed.
        Every Git call is bounded by the absolute deadline on clock(), and none
        starts once too little of it remains. Returns the number removed.
        """
        def within(seconds):
            if deadline is None:
                return seconds
            remaining = deadline - clock()
            require(remaining >= PRUNE_STEP_SECONDS, "Pruning budget expired")
            return min(seconds, remaining)

        identity = receipt["identity"]
        component = identity["component"]
        require(component in DEPLOYABLES, "Retired image tags must remain unchanged")
        finals = self.settled.get(component)
        finals = [tag["receipt"]["identity"] for tag in (self.api.tags(component) if finals is None else finals)]
        history = self.api.history()
        pending, claims = self.advertised(component, timeout=within(30))
        backed = {claim_ref(final) for final in finals}
        # Objects are read for the refs of that advertisement, never a later listing.
        rows = [{"ref": ref, "object": {"type": "tag", "sha": oid}} for ref, oid in pending.items()]
        doomed, removed = {}, 0
        for value, oid in sorted(self.inventory(component, rows), key=lambda entry: version_tuple(entry[0]["version"])):
            if removed == PRUNE_LIMIT:
                break
            if value["run_id"] == identity["run_id"] or source_is_retired(component, value["source"]):
                continue
            if value["source"] in history.positions:
                fenced = any(self.supersedes(final, value, history) for final in finals)
            else:
                # A source that left history can never build; any higher final fences it.
                fenced = any(version_tuple(final["version"]) > version_tuple(value["version"]) for final in finals)
            settled = any(all(final[key] == value[key] for key in OWNER) for final in finals)
            claim = claim_ref(value)
            if not (fenced or settled) or claims.get(claim, oid) != oid:
                continue
            doomed["refs/tags/" + pending_name(value)] = oid
            if claim in claims and claim not in backed:
                doomed[claim] = oid
            removed += 1
        if not doomed:
            return 0
        # Deletions only, never an update. Each lease names the object read
        # above; a moved ref rejects the whole transaction, claims included.
        self.command("push", "--atomic", "--porcelain",
                     *("--force-with-lease=" + ref + ":" + oid for ref, oid in doomed.items()),
                     REMOTE, *(":" + ref for ref in doomed), timeout=within(60))
        # A push cut short may still have landed; only the advertisement settles it.
        pending, claims = self.advertised(component, timeout=within(30))
        require(not (pending.keys() | claims.keys()) & doomed.keys(), "Burned reservation pruning failed")
        return removed

    def finalize(self, receipt, verify_digest, *, recovery_oid=None):
        self.mark("receipt")
        validate_receipt(receipt)
        component = receipt["identity"]["component"]
        require(component in DEPLOYABLES, "Retired image tags must remain unchanged")
        require_publishable_release(receipt["identity"])
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
        self.require_current(expected, build_started=True)
        self.mark("tag_create")
        created = self.command("mktag", data=final_tag_text(receipt))
        require(created.returncode == 0 and matches(r"[0-9a-f]{40}", created.stdout.strip()),
                "Final annotation could not be created")
        expected_oid = created.stdout.strip()
        self.mark("tag_push")
        # The insert-only outcome is written atomically with the final tag.
        # Allocation no longer claims it, since a started build may finish;
        # an older allocator that did claim it still makes this push fail.
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
