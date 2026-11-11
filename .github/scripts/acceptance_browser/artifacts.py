"""Bounded public build/asset attestation; response bytes stay in memory."""

import hashlib
from html.parser import HTMLParser
import json
import re
from urllib.parse import urljoin, urlsplit

from safety import CMA_ORIGIN, Http, Refusal


def digest(value):
    return hashlib.sha256(value).hexdigest()


class AuditedHttp(Http):
    """Keep numeric failures even when an incomplete artifact is refused."""

    def __init__(self):
        super().__init__()
        self.reads = []

    def request(self, origin, path, **kwargs):
        row = {"path_sha256": digest(path.encode()), "status": None, "bytes": None}
        self.reads.append(row)
        result = super().request(origin, path, **kwargs)
        row.update(status=result[0], bytes=len(result[2]))
        return result


class Entries(HTMLParser):
    def __init__(self):
        super().__init__()
        self.paths = []

    def handle_starttag(self, tag, attributes):
        fields = dict(attributes)
        if tag == "script" and fields.get("src"):
            self.paths.append(fields["src"])
        if tag == "link" and fields.get("rel") in ("modulepreload", "stylesheet"):
            self.paths.append(fields.get("href", ""))


def asset_path(raw, parent="/"):
    resolved = urlsplit(urljoin(CMA_ORIGIN + parent, raw))
    if (
        resolved.scheme + "://" + resolved.netloc != CMA_ORIGIN
        or resolved.query
        or resolved.fragment
        or not re.fullmatch(r"/assets/[A-Za-z0-9_.-]+", resolved.path)
    ):
        raise Refusal("asset_reference_not_allowlisted")
    return resolved.path


def attest(http=None):
    http = http or Http()
    info = http.request(CMA_ORIGIN, "/build-info.json")
    if info[0] != 200:
        raise Refusal("build_info_unavailable")
    try:
        build = json.loads(info[2])
    except (ValueError, UnicodeError):
        raise Refusal("build_info_invalid") from None
    if (
        not re.fullmatch(r"[0-9a-f]{40}", str(build.get("revision", "")))
        or build.get("local") is not False
        or not re.fullmatch(
            r"(?:git-[0-9a-f]{12}|v?\d+\.\d+\.\d+)", str(build.get("version", ""))
        )
    ):
        raise Refusal("build_info_invalid")
    root = http.request(CMA_ORIGIN, "/", limit=2 * 1024 * 1024)
    if root[0] != 200:
        raise Refusal("root_unavailable")
    entries = Entries()
    entries.feed(root[2].decode("utf-8"))
    queue = [asset_path(raw) for raw in entries.paths]
    if not queue:
        raise Refusal("entry_assets_missing")
    rows = []
    seen = set()
    total = 0
    while queue:
        path = queue.pop(0)
        if path in seen:
            continue
        if len(seen) >= 256:
            raise Refusal("asset_count_limit")
        seen.add(path)
        response = http.request(CMA_ORIGIN, path, limit=8 * 1024 * 1024)
        total += len(response[2])
        if response[0] != 200 or total > 32 * 1024 * 1024:
            raise Refusal("asset_status_or_size_limit")
        payload = response[2]
        rows.append(
            {
                "path_sha256": digest(path.encode()),
                "body_sha256": digest(payload),
                "bytes": len(payload),
                "status": response[0],
            }
        )
        if path.endswith((".js", ".css")):
            text = payload.decode("utf-8")
            # Vite static/dynamic/preload dependencies, including bare assets/* tables.
            references = re.findall(
                r"""["']((?:\./|/assets/|assets/)[A-Za-z0-9_.-]+\.(?:js|css|woff2?|ttf|otf|png|jpe?g|svg|webp|gif|ico|wasm|avif))["']""",
                text,
            )
            if path.endswith(".css"):
                references.extend(re.findall(r"""url\(["']?([^\s"')]+)["']?\)""", text))
            for reference in references:
                if reference.startswith("data:"):
                    continue
                queue.append(
                    asset_path(
                        "/" + reference
                        if reference.startswith("assets/")
                        else reference,
                        path,
                    )
                )
    # Detect deployment movement across the crawl instead of combining two artifacts.
    end = http.request(CMA_ORIGIN, "/build-info.json")
    if end[0] != 200 or end[2] != info[2]:
        raise Refusal("artifact_changed_during_observation")
    return {
        "revision": build["revision"],
        "version": build["version"],
        "build_info_sha256": digest(info[2]),
        "root_sha256": digest(root[2]),
        "assets": rows,
        "total_bytes": total,
        "entry_count": len(entries.paths),
    }
