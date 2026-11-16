"""Read-only Kubernetes Event classifier. Never treats process exit 0 as hook success."""
import copy
import datetime as dt
import hashlib
import json
import re

SERVICES = {"am", "eg", "web"}
REASONS = ("pre_stop", "post_start", "drain", "backoff")
NAME = re.compile(r"[a-z0-9][a-z0-9.-]{0,252}\Z")


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True).encode()).hexdigest()


def timestamp(value):
    if not isinstance(value, str):
        return None
    try:
        parsed = dt.datetime.fromisoformat(value.replace("Z", "+00:00"))
        return parsed.timestamp() if parsed.tzinfo else None
    except ValueError:
        return None


def utc(value):
    return dt.datetime.fromtimestamp(value, dt.timezone.utc).isoformat()


def target(value):
    parts = value.split(":")
    if len(parts) != 3 or parts[0] not in SERVICES or not all(NAME.fullmatch(s) for s in parts[1:]):
        raise ValueError("target_requires_service_namespace_pod_prefix")
    return dict(zip(("service", "namespace", "pod_prefix"), parts))


def classify(event):
    reason = event.get("reason", "")
    if not isinstance(reason, str):
        raise ValueError("hook_event_reason_invalid")
    if reason == "FailedPreStopHook":
        return "pre_stop"
    if reason == "FailedPostStartHook":
        return "post_start"
    if reason in {"DrainHookFailed", "FailedDrainHook", "DrainFailed", "DrainTimeout"}:
        return "drain"
    message = str(event.get("note", event.get("message", ""))).lower()
    if event.get("type") == "Warning" and re.search(r"\bdrain(?:ing)?\b", message):
        if re.search(r"\b(fail(?:ed|ure)?|error|timeout|timed out|deadline|refused|denied)\b", message):
            return "drain"
    if reason.lower() in {"backoff", "imagepullbackoff", "crashloopbackoff"}:
        return "backoff"
    if event.get("type") == "Warning" and re.search(r"\bback[ -]?off\b", message):
        return "backoff"
    return None


def normalize(event, scope):
    category = classify(event)
    if category is None:
        return None
    ref = event.get("regarding", event.get("involvedObject", {}))
    meta = event.get("metadata", {})
    if not isinstance(ref, dict) or not isinstance(meta, dict):
        raise ValueError("hook_event_identity_invalid")
    namespace = ref.get("namespace", meta.get("namespace"))
    pod = ref.get("name", "")
    if not isinstance(pod, str):
        raise ValueError("hook_event_identity_invalid")
    if ref.get("kind") != "Pod" or namespace != scope["namespace"] or not pod.startswith(scope["pod_prefix"]):
        return None
    if not all(isinstance(v, str) and v for v in (meta.get("uid"), ref.get("uid"), pod)):
        raise ValueError("hook_event_identity_missing")
    series = event.get("series") or {}
    if not isinstance(series, dict):
        raise ValueError("hook_event_series_invalid")
    count = series.get("count", event.get("count", event.get("deprecatedCount", 1)))
    if not isinstance(count, int) or isinstance(count, bool) or count < 1:
        raise ValueError("hook_event_count_invalid")
    times = [series.get("lastObservedTime"), event.get("lastTimestamp"),
             event.get("deprecatedLastTimestamp"), event.get("eventTime"),
             event.get("firstTimestamp"), meta.get("creationTimestamp")]
    observed = next((v for v in map(timestamp, times) if v is not None), None)
    # These are kubelet/API observations, not independent fault-start timestamps.
    return {"key": digest([meta["uid"], ref["uid"], category]),
            "event_uid": meta["uid"], "pod_uid": ref["uid"], "pod": pod,
            "namespace": namespace, "service": scope["service"], "category": category,
            "count": count, "event_observed_at": observed,
            "message_sha256": digest(event.get("note", event.get("message", ""))),
            "scope_proof": "configured_namespace_and_pod_name_prefix"}


class Monitor:
    def __init__(self, targets, since, binding, state=None):
        self.targets, self.since, self.binding = targets, since, binding
        self.state = state if state is not None else {
            "schema": "cma-hook-event-state-v1", "binding": binding,
            "since": since, "seen": {}, "detections": {}, "last_success": {}}
        if not isinstance(self.state, dict):
            raise ValueError("watcher_state_invalid")
        if self.state.get("binding") != binding or self.state.get("schema") != "cma-hook-event-state-v1":
            raise ValueError("watcher_state_binding_mismatch")
        if not isinstance(self.state.get("since"), (int, float)) or not all(
                isinstance(self.state.get(key), dict) for key in ("seen", "detections", "last_success")):
            raise ValueError("watcher_state_invalid")
        for value in self.state["seen"].values():
            if not isinstance(value, dict) or not isinstance(value.get("count"), int) or value["count"] < 1:
                raise ValueError("watcher_state_invalid")
        if any(not isinstance(v, (int, float)) or v < 0 for key in ("detections", "last_success")
               for v in self.state[key].values()):
            raise ValueError("watcher_state_invalid")
        self.since = self.state["since"]
        self.up = {scope["namespace"]: 0 for scope in targets}
        self.coverage_lost = False

    def consume(self, namespace, payload, now):
        # A malformed later row must not consume an earlier detection silently.
        staged = Monitor(self.targets, self.since, self.binding, copy.deepcopy(self.state))
        alerts = staged._consume(namespace, payload, now)
        self.state = staged.state
        self.up[namespace] = 1
        return alerts

    def _consume(self, namespace, payload, now):
        if not isinstance(payload, dict) or payload.get("kind") not in {"EventList", "List"} or not isinstance(payload.get("items"), list):
            raise ValueError("events_list_invalid")
        if not isinstance(payload.get("metadata", {}), dict):
            raise ValueError("events_list_invalid")
        if payload.get("metadata", {}).get("continue"):
            raise ValueError("events_list_incomplete")
        if len(payload["items"]) > 20000:
            raise ValueError("events_list_budget_exceeded")
        alerts = []
        for event in payload["items"]:
            if not isinstance(event, dict):
                raise ValueError("event_invalid")
            for scope in self.targets:
                if scope["namespace"] != namespace:
                    continue
                row = normalize(event, scope)
                if row is None:
                    continue
                at = row["event_observed_at"]
                if at is not None and at > now + 5:
                    raise ValueError("event_clock_ahead")
                if at is not None and at < self.since:
                    continue
                key = scope["service"] + ":" + row["key"]
                previous = self.state["seen"].get(key)
                identity = {"count": row["count"], "at": at}
                if previous == identity:
                    continue
                if previous and row["count"] < previous["count"]:
                    raise ValueError("event_count_regressed")
                if len(self.state["seen"]) >= 20000 and key not in self.state["seen"]:
                    raise ValueError("watcher_state_budget_exceeded")
                self.state["seen"][key] = identity
                metric = scope["service"] + ":" + row["category"]
                self.state["detections"][metric] = self.state["detections"].get(metric, 0) + 1
                alerts.append({"schema": "cma-hook-alert-v1", "severity": "critical", **row,
                               "detected_at": utc(now), "event_observation_to_detection_seconds":
                               None if at is None else max(0, now - at),
                               "fault_to_detection_seconds": None, "delivery": "NOT_SENT",
                               "acknowledged_at": None, "mtta_seconds": None})
        self.up[namespace] = 1
        self.state["last_success"][namespace] = now
        return alerts

    def failure(self, namespace, now, code):
        self.up[namespace] = 0
        self.coverage_lost = True
        return {"schema": "cma-hook-watch-gap-v1", "severity": "critical", "namespace": namespace,
                "detected_at": utc(now), "code": code, "delivery": "NOT_SENT",
                "last_success_at": self.state["last_success"].get(namespace),
                "mtta_seconds": None}

    def metrics(self, now):
        lines = ["# TYPE cma_hook_event_detections_total counter"]
        for service in sorted({t["service"] for t in self.targets}):
            for reason in REASONS:
                value = self.state["detections"].get(service + ":" + reason, 0)
                lines.append(f'cma_hook_event_detections_total{{service="{service}",reason="{reason}"}} {value}')
        for metric in ("cma_hook_event_watch_up", "cma_hook_event_last_success_timestamp_seconds"):
            lines.append(f"# TYPE {metric} gauge")
            for namespace in sorted(self.up):
                value = self.up[namespace] if metric.endswith("_up") else self.state["last_success"].get(namespace, 0)
                lines.append(f'{metric}{{namespace="{namespace}"}} {value}')
        lines += ["# TYPE cma_hook_event_watch_timestamp_seconds gauge",
                  f"cma_hook_event_watch_timestamp_seconds {now}"]
        return "\n".join(lines) + "\n"
