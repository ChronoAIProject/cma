"""Read BuildKit index contents and report unreachable cache layers."""
import hashlib
import json
import os
import re
import sys
from collections import Counter, defaultdict
from concurrent.futures import ThreadPoolExecutor
from contextlib import contextmanager
from dataclasses import asdict, dataclass, replace
from datetime import datetime, timedelta, timezone
from pathlib import Path
from urllib.parse import urlsplit
from urllib.request import HTTPRedirectHandler, Request, build_opener

from actions_cache_retention import DEVELOP, Entry
from actions_cache_sweep import Actions
from staging_cd_build_cache import CI_SCOPES, SCOPES


CACHE_VERSION = hashlib.sha256(b"|go-actionscache-1.0").hexdigest()
INDEX_PREFIX = "index-"
BLOB_PREFIX = "buildkit-blob-1-sha256:"
INDEX = re.compile(r"index-[a-z0-9-]+-1-[a-f0-9]{8}(?:-sig)?#[1-9][0-9]*")
DIGEST = r"sha256:[a-f0-9]{64}"
BLOB = re.compile("buildkit-blob-1-(" + DIGEST + ")")
RESULTS_LOOKUP = "/twirp/github.actions.results.api.v1.CacheService/GetCacheEntryDownloadURL"
MAX_INDEXES = 1000
MAX_INDEX_RETIREMENTS = 8
MAX_INDEX_BYTES = 1024 * 1024
DEVELOP_SCOPE = hashlib.sha256(DEVELOP.encode()).hexdigest()[:8]
MANAGED_INDEX_STEMS = frozenset(
    f"index-{scope}-1-{DEVELOP_SCOPE}" for scope in CI_SCOPES.union(SCOPES.values())
)


class ProofError(ValueError):
    """Identify the failed observation without printing remote response content."""

    def __init__(self, phase, cause):
        self.phase = phase
        self.cause_type = type(cause).__name__
        super().__init__(str(cause))


@contextmanager
def observing(phase):
    try:
        yield
    except Exception as error:
        raise ProofError(phase, error) from error


class RejectRedirect(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def distinct_fields(pairs):
    fields, result = set(), {}
    for key, value in pairs:
        folded = key.casefold()
        if folded in fields:
            raise ValueError("ambiguous BuildKit JSON fields")
        fields.add(folded)
        result[key] = value
    return result


def read_json(request, limit, *, expected_size=None):
    with build_opener(RejectRedirect()).open(request, timeout=15) as response:
        raw = response.read(limit + 1)
    if len(raw) > limit:
        raise ValueError("cache response exceeds size limit")
    if expected_size is not None and len(raw) != expected_size:
        raise ValueError("BuildKit download differs from inventory size")
    return json.loads(raw, object_pairs_hook=distinct_fields)


def indexes(entries):
    found = [entry for entry in entries if entry.version == CACHE_VERSION and entry.key.startswith("index-")]
    if (not found or len(found) > MAX_INDEXES
            or any(entry.ref != DEVELOP or not INDEX.fullmatch(entry.key)
                   or not 0 < entry.size <= MAX_INDEX_BYTES for entry in found)
            or len({entry.key for entry in found}) != len(found)):
        raise ValueError("unsupported BuildKit index inventory")
    # Reading an index can advance its access timestamp without changing its identity.
    return {entry.id: replace(entry, accessed=entry.created) for entry in found}


def valid_record(record, layer_count):
    if not isinstance(record, dict) or record.keys() - {"layers", "chains", "digest", "inputs"}:
        return False
    direct, chains = record.get("layers", []), record.get("chains", [])
    if not isinstance(direct, list) or not isinstance(chains, list):
        return False
    for row in direct:
        if not isinstance(row, dict) or row.keys() - {"layer", "createdAt"}:
            return False
        # BuildKit's omitempty encoding represents layer zero by omission.
        value = row.get("layer", 0)
        if type(value) is not int or not 0 <= value < layer_count:
            return False
    for row in chains:
        if not isinstance(row, dict) or row.keys() - {"layers", "createdAt"}:
            return False
        selected = row.get("layers", [])
        if (not isinstance(selected, list)
                or any(type(value) is not int or not 0 <= value < layer_count for value in selected)):
            return False
    return True


def layers(document):
    if not isinstance(document, dict) or document.keys() - {"layers", "records"}:
        raise ValueError("unsupported BuildKit index format")
    rows = document.get("layers", [])
    records = document.get("records", [])
    if not isinstance(rows, list) or not isinstance(records, list):
        raise ValueError("invalid BuildKit layers")
    result = set()
    for row in rows:
        if not isinstance(row, dict) or row.keys() - {"blob", "parent", "annotations"}:
            raise ValueError("unsupported BuildKit layer fields")
        digest = row.get("blob")
        if not isinstance(digest, str) or not re.fullmatch(DIGEST, digest):
            raise ValueError("invalid BuildKit layer digest")
        parent = row.get("parent", 0)
        if type(parent) is not int or not -1 <= parent < len(rows):
            raise ValueError("unknown BuildKit parent layer")
        result.add("buildkit-blob-1-" + digest)
    if not all(valid_record(record, len(rows)) for record in records):
        raise ValueError("unresolved BuildKit record layers")
    return result


def read_index(results_url, token, key, expected_size):
    body = json.dumps({"key": key, "restore_keys": [key], "version": CACHE_VERSION}).encode()
    reply = read_json(Request(results_url + RESULTS_LOOKUP, data=body,
                              headers={"Authorization": "Bearer " + token, "Content-Type": "application/json"}),
                      32 * 1024)
    if reply.get("ok") is not True or reply.get("matched_key") != key:
        raise ValueError("exact BuildKit index was unavailable")
    signed_url = reply["signed_download_url"]
    url = urlsplit(signed_url)
    if (url.scheme != "https" or not (url.hostname or "").endswith(".blob.core.windows.net")
            or url.username or url.password or url.fragment or url.port not in (None, 443)):
        raise ValueError("unexpected BuildKit index download endpoint")
    return layers(read_json(signed_url, MAX_INDEX_BYTES, expected_size=expected_size))


def entries_with_prefix(api, prefix, **filters):
    entries = api.entries(key=prefix, sort="created_at", direction="desc", **filters)
    if any(not entry.key.startswith(prefix) for entry in entries):
        raise ValueError("cache response included a key outside the requested prefix")
    return entries


def develop_blobs(api):
    # A mutation only forces a pagination retry in its own digest partition.
    entries = {}
    for digit in "0123456789abcdef":
        for entry in entries_with_prefix(api, BLOB_PREFIX + digit, ref=DEVELOP):
            if entry.ref != DEVELOP or not BLOB.fullmatch(entry.key) or entry.id in entries:
                raise ValueError("invalid BuildKit blob inventory")
            entries[entry.id] = entry
    return entries


def superseded_indexes(pinned, references, stable_keys, cutoff):
    groups = defaultdict(list)
    for row in pinned.values():
        stem, _, generation = row.key.rpartition("#")
        if stem in MANAGED_INDEX_STEMS:
            groups[stem].append((int(generation), row))
    retired = []
    for group in groups.values():
        if len(group) < 3:
            continue
        ordered = sorted(group, key=lambda item: (item[1].created, item[1].id))
        generations = [generation for generation, _ in ordered]
        if generations != sorted(generations):
            # A reset or out-of-order upload makes the lookup order ambiguous.
            continue
        keep = [references[row.id] for _, row in ordered[-2:]]
        if not all(keys and keys <= stable_keys for keys in keep):
            continue
        retired.extend(row for _, row in ordered[:-2] if row.created <= cutoff)
    return tuple(sorted(retired, key=lambda row: (row.created, row.id)))


def retirement_batch(aged, references, superseded, *, target=None):
    """Prioritize reference sets that can make the most aged bytes unreachable."""
    order = {row.id: index for index, row in enumerate(superseded)}
    aged_bytes = Counter()
    for row in aged:
        aged_bytes[row.key] += row.size
    owners = defaultdict(set)
    for index_id, keys in references.items():
        for key in keys & aged_bytes.keys():
            owners[key].add(index_id)

    # A blob only contributes once the batch removes all of its references.
    # Any surviving, unmanaged or newer index excludes it from this calculation.
    weights = Counter()
    for key, ids in owners.items():
        if ids <= order.keys():
            weights[frozenset(ids)] += aged_bytes[key]

    selected = set()
    projected = 0
    while len(selected) < MAX_INDEX_RETIREMENTS and (target is None or projected < target):
        remaining = Counter()
        for group, size in weights.items():
            missing = group - selected
            if missing:
                remaining[missing] += size
        if not remaining:
            break

        slots = MAX_INDEX_RETIREMENTS - len(selected)
        # Prefer a set we can finish now. If none fits, advance one set across
        # multiple independent observations instead of scattering removals.
        fitting = [group for group in remaining if len(group) <= slots]
        if not fitting and selected:
            break

        def priority(group):
            count, size = len(group), remaining[group]
            enough = target is not None and count <= slots and size >= target - projected
            order_score = tuple(-index for index in sorted(order[member] for member in group))
            return enough, -count if enough else size // count, size, order_score

        best = max(fitting or remaining, key=priority)
        selected.update(sorted(best, key=order.__getitem__)[:slots])
        projected = sum(size for group, size in weights.items() if group <= selected)
    return tuple(row for row in superseded if row.id in selected), projected, weights.total()


@dataclass(frozen=True)
class Observation:
    pinned: dict[int, Entry]
    candidates: tuple[Entry, ...]
    superseded: tuple[Entry, ...]
    retirement_batch: tuple[Entry, ...]
    projected_newly_unreachable_bytes: int
    projected_candidate_bytes: int
    observed_blob_bytes: int
    reachable_layer_count: int


def observe(api, reader, revision, now, *, retirement_target=None):
    with observing("develop-before"):
        api.require_live_policy(revision)
    # The exact index key/version must be unique across all refs. Only blob
    # candidates are scoped to develop.
    with observing("indexes-before"):
        pinned = indexes(entries_with_prefix(api, INDEX_PREFIX))
    with observing("blobs-before"):
        before = develop_blobs(api)
    with observing("index-content"):
        with ThreadPoolExecutor(max_workers=8) as pool:
            references = dict(zip(pinned, pool.map(lambda row: reader(row.key, row.size), pinned.values())))
    with observing("blobs-after"):
        after = develop_blobs(api)
    with observing("indexes-after"):
        if indexes(entries_with_prefix(api, INDEX_PREFIX)) != pinned:
            raise ValueError("BuildKit indexes changed during inventory")
    with observing("develop-after"):
        api.require_live_policy(revision)
    cutoff = now - timedelta(hours=24)
    stable = [row for row in after.values() if row.size and row.version == CACHE_VERSION and row.id in before
              and replace(row, accessed=before[row.id].accessed) == before[row.id]]
    aged = [row for row in stable if row.accessed <= cutoff and row.created <= cutoff and row == before[row.id]]
    reachable = set().union(*references.values())
    candidates = tuple(sorted((row for row in aged if row.key not in reachable),
                              key=lambda row: (-row.size, row.id)))
    superseded = superseded_indexes(pinned, references, {row.key for row in stable}, cutoff)
    batch, newly_unreachable, all_newly_unreachable = retirement_batch(
        aged, references, superseded, target=retirement_target,
    )
    projected = sum(row.size for row in candidates) + all_newly_unreachable
    return Observation(pinned, candidates, superseded, batch, newly_unreachable, projected,
                       sum(row.size for row in after.values()), len(reachable))


def snapshot(api, reader, revision, now):
    observation = observe(api, reader, revision, now)
    return {"develop": revision, "observed_at": now.isoformat(), "index_count": len(observation.pinned),
            "reachable_layer_count": observation.reachable_layer_count,
            "observed_develop_blob_bytes": observation.observed_blob_bytes,
            "candidate_bytes": sum(entry.size for entry in observation.candidates),
            "candidates": [asdict(entry) for entry in observation.candidates],
            "superseded_index_count": len(observation.superseded),
            "next_index_batch_count": len(observation.retirement_batch),
            "projected_newly_unreachable_bytes": observation.projected_newly_unreachable_bytes,
            "projected_candidate_bytes_after_index_retirement": observation.projected_candidate_bytes}


def client(env):
    results_url = env["ACTIONS_RESULTS_URL"].rstrip("/")
    url = urlsplit(results_url)
    if (url.scheme != "https" or url.hostname != "results-receiver.actions.githubusercontent.com"
            or url.username or url.password or url.path or url.query or url.fragment or url.port not in (None, 443)):
        raise ValueError("unexpected Actions Results endpoint")
    api = Actions(env["GH_TOKEN"], int(env["GITHUB_RUN_ID"]))
    return api, lambda key, size: read_index(results_url, env["ACTIONS_RUNTIME_TOKEN"], key, size)


def main(path, env):
    api, reader = client(env)
    result = snapshot(api, reader, env["GITHUB_SHA"], datetime.now(timezone.utc))
    Path(path).write_text(json.dumps(result, default=str) + "\n")
    print(f"BuildKit indexes={result['index_count']} candidate_bytes={result['candidate_bytes']} "
          f"superseded_indexes={result['superseded_index_count']} "
          f"projected_candidate_bytes={result['projected_candidate_bytes_after_index_retirement']}")


if __name__ == "__main__":
    try:
        main(sys.argv[1], os.environ)
    except ProofError as error:
        print(f"BuildKit cache inventory failed at {error.phase} ({error.cause_type})", file=sys.stderr)
        sys.exit(1)
    except Exception as error:
        print(f"BuildKit cache inventory failed ({type(error).__name__})", file=sys.stderr)
        sys.exit(1)
