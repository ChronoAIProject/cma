"""Read BuildKit index contents and report unreachable cache layers."""
import hashlib
import json
import os
import re
import sys
from concurrent.futures import ThreadPoolExecutor
from dataclasses import asdict
from datetime import datetime, timedelta, timezone
from pathlib import Path
from urllib.parse import urlsplit
from urllib.request import HTTPRedirectHandler, Request, build_opener

from actions_cache_retention import DEVELOP
from actions_cache_sweep import Actions


CACHE_VERSION = hashlib.sha256(b"|go-actionscache-1.0").hexdigest()
INDEX = re.compile(r"index-[a-z0-9-]+-1-[a-f0-9]{8}(?:-sig)?#[1-9][0-9]*")
DIGEST = r"sha256:[a-f0-9]{64}"
BLOB = re.compile("buildkit-blob-1-(" + DIGEST + ")")
RESULTS_LOOKUP = "/twirp/github.actions.results.api.v1.CacheService/GetCacheEntryDownloadURL"
MAX_INDEXES = 1000
MAX_INDEX_BYTES = 1024 * 1024


class RejectRedirect(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def read_json(request, limit):
    with build_opener(RejectRedirect()).open(request, timeout=15) as response:
        raw = response.read(limit + 1)
    if len(raw) > limit:
        raise ValueError("cache response exceeds size limit")
    return json.loads(raw)


def indexes(entries):
    found = [entry for entry in entries if entry.version == CACHE_VERSION and entry.key.startswith("index-")]
    if (not found or len(found) > MAX_INDEXES
            or any(entry.ref != DEVELOP or not INDEX.fullmatch(entry.key)
                   or not 0 < entry.size <= MAX_INDEX_BYTES for entry in found)
            or len({entry.key for entry in found}) != len(found)):
        raise ValueError("unsupported BuildKit index inventory")
    return {entry.id: (entry.key, entry.ref, entry.version, entry.size, entry.created) for entry in found}


def layers(document):
    if not isinstance(document, dict) or document.keys() - {"layers", "records"}:
        raise ValueError("unsupported BuildKit index format")
    rows = document.get("layers", [])
    if not isinstance(rows, list):
        raise ValueError("invalid BuildKit layers")
    result = set()
    for row in rows:
        digest = row.get("blob") if isinstance(row, dict) else None
        if not isinstance(digest, str) or not re.fullmatch(DIGEST, digest):
            raise ValueError("invalid BuildKit layer digest")
        result.add("buildkit-blob-1-" + digest)
    return result


def read_index(results_url, token, key):
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
    return layers(read_json(signed_url, MAX_INDEX_BYTES))


def snapshot(api, reader, revision, now):
    api.require_live_policy(revision)
    before = api.entries(sort="created_at", direction="desc")
    pinned = indexes(before)
    with ThreadPoolExecutor(max_workers=8) as pool:
        reachable = set().union(*pool.map(reader, sorted(value[0] for value in pinned.values())))
    after = api.entries(sort="created_at", direction="desc")
    if indexes(after) != pinned:
        raise ValueError("BuildKit indexes changed during inventory")
    api.require_live_policy(revision)
    unchanged = {entry.id: entry for entry in before}
    cutoff = now - timedelta(hours=24)
    candidates = [entry for entry in after if entry.ref == DEVELOP and entry.version == CACHE_VERSION
                  and BLOB.fullmatch(entry.key) and entry.key not in reachable and entry.accessed <= cutoff
                  and unchanged.get(entry.id) == entry]
    return {"develop": revision, "observed_at": now.isoformat(), "index_count": len(pinned),
            "reachable_layer_count": len(reachable), "observed_bytes": sum(entry.size for entry in after),
            "candidate_bytes": sum(entry.size for entry in candidates),
            "candidates": [asdict(entry) for entry in sorted(candidates, key=lambda entry: (-entry.size, entry.id))]}


def main(path, env):
    results_url = env["ACTIONS_RESULTS_URL"].rstrip("/")
    url = urlsplit(results_url)
    if (url.scheme != "https" or url.hostname != "results-receiver.actions.githubusercontent.com"
            or url.username or url.password or url.path or url.query or url.fragment or url.port not in (None, 443)):
        raise ValueError("unexpected Actions Results endpoint")
    api = Actions(env["GH_TOKEN"], int(env["GITHUB_RUN_ID"]))
    result = snapshot(api, lambda key: read_index(results_url, env["ACTIONS_RUNTIME_TOKEN"], key),
                      env["GITHUB_SHA"], datetime.now(timezone.utc))
    Path(path).write_text(json.dumps(result, default=str) + "\n")
    print(f"BuildKit indexes={result['index_count']} candidate_bytes={result['candidate_bytes']}")


if __name__ == "__main__":
    try:
        main(sys.argv[1], os.environ)
    except Exception as error:
        print(f"BuildKit cache inventory failed ({type(error).__name__})", file=sys.stderr)
        sys.exit(1)
