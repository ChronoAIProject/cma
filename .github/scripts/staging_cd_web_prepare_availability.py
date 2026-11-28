"""Check first-attempt asset availability at the edge during Web prepare."""

import argparse
from collections import Counter
from concurrent.futures import as_completed, ThreadPoolExecutor
from functools import partial
import http.client
import json
import math
from pathlib import Path
from threading import Event
import time

from staging_cd_common import Refusal, matches, require
from staging_cd_receipts import sha256
from staging_cd_web_continuity import ASSET, HASH, immutable_image
from staging_cd_web_rollout_preflight import read


CATALOG_LIMIT = 2 * 1024**2
ASSET_LIMIT = 2 * 1024**3


def validate_catalog(catalog, digest):
    require(isinstance(catalog, dict) and catalog and matches(HASH, digest),
            "Web asset catalog is missing or malformed")
    require(all(isinstance(name, str) and matches(ASSET, "/assets/" + name) and isinstance(asset, dict)
                and set(asset) == {"bytes", "sha256"}
                and type(asset["bytes"]) is int and 0 <= asset["bytes"] <= ASSET_LIMIT
                and matches(HASH, asset["sha256"]) for name, asset in catalog.items()),
            "Web asset catalog entry is malformed")
    require(sha256(catalog) == digest, "Web asset catalog does not match its release digest")


def remaining(deadline):
    seconds = deadline - time.monotonic()
    require(seconds > 0, "Web prepare availability deadline expired")
    return seconds


def probe(origin, entry, *, deadline, fetch, started):
    name, asset = entry
    path = "/assets/" + name
    observed = []
    timeout = min(10, deadline - time.monotonic())
    if timeout <= 0:
        return None
    started.set()
    try:
        status, _, _, body = fetch(origin, path, evidence=observed, timeout=timeout,
                                   response_limit=asset["bytes"])
        result = "ok"
        if status != 200:
            result = "http-error"
        elif len(body) != asset["bytes"] or observed[0]["body_sha256"] != asset["sha256"]:
            result = "asset-mismatch"
    except Refusal:
        result = "invalid-response"
    except (OSError, http.client.HTTPException):
        result = "transport-error"
    row = observed[0] if observed else {"path": path}
    return row | {"result": result, "expected_sha256": asset["sha256"]}


def watch_prepare(origin, incumbent, prepare, *, incumbent_sha256, prepare_sha256,
                  incumbent_image, prepare_image, deadline, rollout, record, fetch=read):
    """Bracket a caller's submit-and-wait operation in one monotonic budget.

    The caller owns the generation and settled/Ready fences. The same edge origin
    is used throughout. Until stock leaves eligibility, only the incumbent
    catalog is guaranteed on every replica.
    """
    require(math.isfinite(deadline), "Web prepare availability needs a finite deadline")
    require(immutable_image(incumbent_image) and immutable_image(prepare_image) and incumbent_image != prepare_image,
            "Web prepare needs distinct digest-pinned images")
    validate_catalog(incumbent, incumbent_sha256)
    validate_catalog(prepare, prepare_sha256)
    require(all(prepare.get(name) == asset for name, asset in incumbent.items()),
            "Web prepare catalog does not preserve the incumbent catalog")
    record({"type": "start", "incumbent_sha256": incumbent_sha256, "prepare_sha256": prepare_sha256,
            "incumbent_image": incumbent_image, "prepare_image": prepare_image})
    counts = {phase: Counter() for phase in ("stock", "overlap", "prepare")}
    started, settled = Event(), Event()

    with ThreadPoolExecutor(max_workers=8) as requests:
        def sweep(phase, catalog, number):
            attempt = partial(probe, origin, deadline=deadline, fetch=fetch, started=started)
            futures = [requests.submit(attempt, entry) for entry in catalog.items()]
            for future in as_completed(futures):
                row = future.result()
                if row is None:
                    continue
                tally = counts[phase]
                tally["attempts"] += 1
                tally["failures"] += row["result"] != "ok"
                tally["http_404"] += row.get("status") == 404
                tally["http_503"] += row.get("status") == 503
                record(row | {"type": "asset", "phase": phase, "sweep": number})
            remaining(deadline)

        def sample_overlap():
            number = 1
            while True:
                sweep("overlap", incumbent, number)
                if settled.is_set():
                    return
                number += 1

        passed = False
        try:
            sweep("stock", incumbent, 1)
            require(not counts["stock"]["failures"], "Web stock asset availability failed")
            started.clear()
            with ThreadPoolExecutor(max_workers=1) as monitor:
                overlap = monitor.submit(sample_overlap)
                try:
                    if not started.wait(remaining(deadline)):
                        overlap.result()
                        raise Refusal("Web overlap sampling did not start")
                    rollout()
                finally:
                    settled.set()
                overlap.result()
            sweep("prepare", prepare, 1)
            require(not any(tally["failures"] for tally in counts.values()),
                    "Web prepare asset availability failed")
            passed = True
        finally:
            phases = {phase: {key: tally[key] for key in ("attempts", "failures", "http_404", "http_503")}
                      for phase, tally in counts.items()}
            record({"type": "result", "passed": passed, "phases": phases})


def read_catalog(path):
    with path.open("rb") as stream:
        payload = stream.read(CATALOG_LIMIT + 1)
    require(0 < len(payload) <= CATALOG_LIMIT, "Web asset catalog is missing or oversized")
    return json.loads(payload)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--origin", required=True)
    for phase in ("incumbent", "prepare"):
        parser.add_argument(f"--{phase}-catalog", type=Path, required=True)
        parser.add_argument(f"--{phase}-sha256", required=True, help="Digest from the release catalog")
        parser.add_argument(f"--{phase}-image", required=True, help="Digest-pinned Web image")
    parser.add_argument("--ready-file", type=Path, required=True,
                        help="Created by this check when overlap sampling starts. Submit prepare after it appears.")
    parser.add_argument("--settled-file", type=Path, required=True,
                        help="Atomically write 'prepared' after verifying prepare is serving and stock has withdrawn. "
                             "Write 'rollback' when the rollout is recovered instead.")
    parser.add_argument("--evidence", type=Path, required=True, help="New first-attempt JSONL journal")
    parser.add_argument("--seconds", type=int, required=True, help="Total stock, overlap and prepare time budget")
    args = parser.parse_args()
    deadline = time.monotonic() + args.seconds

    def await_prepare():
        args.ready_file.touch(exist_ok=False)
        while not args.settled_file.is_file():
            time.sleep(min(0.1, remaining(deadline)))
        with args.settled_file.open("r", encoding="utf-8") as status:
            require(status.read(16).strip() == "prepared", "Web prepare rollout did not settle")

    try:
        require(len({path.resolve() for path in (args.ready_file, args.settled_file, args.evidence)}) == 3
                and not args.ready_file.exists() and not args.settled_file.exists(),
                "Web prepare requires fresh, distinct evidence and handoff files")
        with args.evidence.open("x", encoding="utf-8", buffering=1) as journal:
            def record(row):
                journal.write(json.dumps(row, separators=(",", ":")) + "\n")

            watch_prepare(args.origin, read_catalog(args.incumbent_catalog), read_catalog(args.prepare_catalog),
                          incumbent_sha256=args.incumbent_sha256, prepare_sha256=args.prepare_sha256,
                          incumbent_image=args.incumbent_image, prepare_image=args.prepare_image,
                          deadline=deadline, rollout=await_prepare, record=record)
    except Refusal as error:
        parser.exit(1, f"Web prepare availability refused at {error.site}: {error}\n")
    except (OSError, ValueError):
        parser.exit(1, "Web prepare availability could not read or record its inputs\n")
    print("PASS Web prepare asset availability")


if __name__ == "__main__":
    main()
