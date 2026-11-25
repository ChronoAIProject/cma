"""Read BuildKit index contents and report unreachable cache layers."""
import hashlib
import json
import os
import re
import sys
from concurrent.futures import ThreadPoolExecutor
from contextlib import contextmanager
from dataclasses import asdict, replace
from datetime import datetime, timedelta, timezone
from pathlib import Path
from urllib.parse import urlsplit
from urllib.request import HTTPRedirectHandler, Request, build_opener

from actions_cache_retention import DEVELOP
from actions_cache_sweep import Actions


CACHE_VERSION = hashlib.sha256(b"|go-actionscache-1.0").hexdigest()
INDEX_PREFIX = "index-"
BLOB_PREFIX = "buildkit-blob-1-sha256:"
INDEX = re.compile(r"index-[a-z0-9-]+-1-[a-f0-9]{8}(?:-sig)?#[1-9][0-9]*")
DIGEST = r"sha256:[a-f0-9]{64}"
BLOB = re.compile("buildkit-blob-1-(" + DIGEST + ")")
RESULTS_LOOKUP = "/twirp/github.actions.results.api.v1.CacheService/GetCacheEntryDownloadURL"
MAX_INDEXES = 1000
MAX_INDEX_BYTES = 1024 * 1024


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


def snapshot(api, reader, revision, now):
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
            reachable = set().union(*pool.map(lambda row: reader(row.key, row.size), pinned.values()))
    with observing("blobs-after"):
        after = develop_blobs(api)
    with observing("indexes-after"):
        if indexes(entries_with_prefix(api, INDEX_PREFIX)) != pinned:
            raise ValueError("BuildKit indexes changed during inventory")
    with observing("develop-after"):
        api.require_live_policy(revision)
    cutoff = now - timedelta(hours=24)
    candidates = [entry for entry in after.values() if entry.version == CACHE_VERSION
                  and entry.key not in reachable and entry.accessed <= cutoff
                  and before.get(entry.id) == entry]
    return {"develop": revision, "observed_at": now.isoformat(), "index_count": len(pinned),
            "reachable_layer_count": len(reachable),
            "observed_develop_blob_bytes": sum(entry.size for entry in after.values()),
            "candidate_bytes": sum(entry.size for entry in candidates),
            "candidates": [asdict(entry) for entry in sorted(candidates, key=lambda entry: (-entry.size, entry.id))]}


def main(path, env):
    results_url = env["ACTIONS_RESULTS_URL"].rstrip("/")
    url = urlsplit(results_url)
    if (url.scheme != "https" or url.hostname != "results-receiver.actions.githubusercontent.com"
            or url.username or url.password or url.path or url.query or url.fragment or url.port not in (None, 443)):
        raise ValueError("unexpected Actions Results endpoint")
    api = Actions(env["GH_TOKEN"], int(env["GITHUB_RUN_ID"]))
    result = snapshot(api, lambda key, size: read_index(results_url, env["ACTIONS_RUNTIME_TOKEN"], key, size),
                      env["GITHUB_SHA"], datetime.now(timezone.utc))
    Path(path).write_text(json.dumps(result, default=str) + "\n")
    print(f"BuildKit indexes={result['index_count']} candidate_bytes={result['candidate_bytes']}")


if __name__ == "__main__":
    try:
        main(sys.argv[1], os.environ)
    except ProofError as error:
        print(f"BuildKit cache inventory failed at {error.phase} ({error.cause_type})", file=sys.stderr)
        sys.exit(1)
    except Exception as error:
        print(f"BuildKit cache inventory failed ({type(error).__name__})", file=sys.stderr)
        sys.exit(1)
