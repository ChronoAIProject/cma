# Kubernetes lifecycle-hook event observation

The application log stream cannot establish that a Kubernetes lifecycle hook
succeeded. A main process can exit zero while kubelet reports
`FailedPreStopHook`, and the Pod can disappear before a log collector joins the
failure. The bounded reader in `.github/scripts/cma-observability/` observes
Events independently of current Pod existence. It is an operator tool, not a
new deployed collector. Merging it does not activate scraping or paging.

## Run contract

Use the existing observation owner and a private output directory:

```sh
python3 .github/scripts/cma-observability/watch_hook_events.py \
  --context '<explicit-context>' \
  --target am:chrono-bot:cma-agent-manager- \
  --target eg:chrono-bot:cma-event-gateway- \
  --output /private/observation/hook-events \
  --interval 5 --duration 900 --lookback 3600
```

Select the actual namespace and Pod-name prefix from reviewed deployment
metadata. Prefixes are an explicit scope filter; they do not verify a Pod's
ownerReferences. Include an explicit `web:<namespace>:<prefix>` target when
observing web. At most three namespaces and twelve targets are accepted. Keep
expected scope fixed for the observation, including departing Pods; do not
recompute it from only the surviving Ready Pods.

The only Kubernetes command is `get events` with a Pod involved-object filter,
chunking and a five-second API timeout. The subprocess has an eight-second
bound. There are no pod exec, log, Secret, mutation, repair or notification
commands. An existing reader must retain custody; do not launch a competing
reader during a governed rollout. Default cadence is one read cycle then a
five-second delay; the read duration adds to the interval. The default run is
bounded to fifteen minutes and can be extended explicitly to at most one day.
Provide a successor before it ends if observation must continue.

The reader recognizes `FailedPreStopHook`, `FailedPostStartHook`, explicit drain
failure/timeout reasons, and Warning messages containing both drain/draining
and a failure classification. It supports core/v1 and events.k8s.io/v1 payloads,
including aggregated series counts. Kubernetes Events are best effort and may
expire or be rate limited: absence of an Event does not prove successful drain,
release of ownership, or completion of a native side effect. Hooks that never
publish an Event still need independent drain-status and timeout evidence.

## Receipts, freshness and deduplication

`alerts.jsonl` and stdout contain sanitized detections and observer failures.
They retain event UID, Pod UID, namespace, Pod name, count, classification and
observation time. Raw hook commands and messages are omitted; a message hash
is retained. Files are private by default. A successful event receipt is flushed
before the persistent state checkpoint. A crash at that boundary may repeat a
receipt, so downstream consumers must deduplicate its event/pod/category/count
identity too. Restart state is bound to context, targets and live/replay mode.

A repeated UID/count/timestamp is suppressed. A count or timestamp update is a
new observation; a recreated Pod with the same name and different UID remains
distinct. The lookback applies on first start and is retained in the state.
Malformed responses, truncated lists, API errors and state/budget failures do
not produce a healthy receipt. The expected namespace remains in the output
with health zero and its previous successful-read timestamp. Stop and inspect
a state failure instead of silently resetting the deduplication history.

`status.json` records each expected namespace, source mode and any gap during
the run. `metrics.prom` is an atomic Prometheus textfile for the existing
collector's supported textfile input; there is no additional HTTP listener.
Its ingestion and alert rules must be configured in the platform collector:

| Metric | Meaning |
| --- | --- |
| `cma_hook_event_detections_total{service,reason}` | New or updated Event observations, not the Event's aggregated number of failures. Reasons are `pre_stop`, `post_start`, `drain`; services are `am`, `eg`, `web`. |
| `cma_hook_event_watch_up{namespace}` | Last namespace read was complete and parseable. Expected namespaces remain present on failure. |
| `cma_hook_event_last_success_timestamp_seconds{namespace}` | Time of the last complete read; unchanged on failure. |
| `cma_hook_event_watch_timestamp_seconds` | Last output cycle. An old timestamp exposes a stopped watcher even if its last health value is one. |

An increase in detections is a critical hook failure; health zero or a missing/
stale timestamp is a separate critical observation gap. Establish the expected
watcher and namespace denominator outside these series so deletion of all
series cannot look healthy. Restarted output directories reset counters; retain
the same state directory or make a recorded observation-generation transition.

Exit codes are `0` (no new detection/gap in this run), `1` (hook failure), `2`
(observation gap/failure) and `130` (interrupted). **Exit zero is never a clean
rollout or clean-ownership-release verdict.** Previous detections remain in the
journal and counters even when a restart has no new detection.

The receipt's Event-observation-to-detection age uses the kubelet/API timestamp,
not an independently measured fault start. Missing timestamps retain unknown
age, and future timestamps beyond five seconds are a coverage failure. Offline
replay age is historical age, never live detection latency. All receipts state
`delivery: NOT_SENT`; human recipients, actual page delivery and MTTA remain
unmeasured until the existing alert pipeline is joined and exercised.

## Offline validation

```sh
python3 -m unittest discover \
  -s .github/scripts/cma-observability -p 'test_*.py' -v
python3 .github/scripts/cma-observability/watch_hook_events.py \
  --context saved-rehearsal \
  --target am:cma-native-am:am-matrix- \
  --output /private/observation/replay \
  --lookback 86400 --replay /private/evidence/events.json
```

Replay makes no network calls. Tests cover the deleted-Pod failure, UID changes,
series updates, state reuse, malformed and incomplete input, retained no-data
denominators, missing/future timestamps, read errors, private message exclusion
and exact read-only command construction. Before declaring M-OBS-01 PASS,
retain actual fault, Event, detection, platform delivery and human acknowledgment
receipts in an authorized rehearsal, plus the required seven-day collection
coverage. A local source/test PASS cannot replace those receipts.
