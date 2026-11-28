"""Qualify a Web route before exposure and verify an isolated drain witness."""

import argparse
from datetime import datetime, timezone
from hashlib import sha256
import http.client
import json
from pathlib import Path
from urllib.parse import urlsplit
from uuid import uuid4

from staging_cd_common import Refusal, matches, require
from staging_cd_set import document
from staging_cd_versions import version_tuple
from staging_cd_web_continuity import ASSET, HASH, immutable_image


READY_PATH = "/web-readyz"
NGINX_PATH = "/etc/nginx/conf.d/default.conf"
HOOK_PATH = "/usr/local/bin/cma-web-prestop"
HOOK_SOURCE = Path(__file__).resolve().parents[2] / "web/scripts/web-prestop.sh"
PROBE_PATHS = {"readiness": READY_PATH, "startup": "/", "liveness": "/"}
MAX_RESPONSE_BYTES = 2 * 1024**2
MAX_INPUT_BYTES = 65536


def validate_service(service, image, nginx_sha256):
    require(isinstance(service, dict) and immutable_image(image) and service.get("image") == image,
            "Effective Web service image differs from the selected digest")
    probes = service.get("probes")
    require(service.get("health_path") == READY_PATH and isinstance(probes, dict)
            and all(isinstance(probes.get(name), dict) and probes[name].get("path") == path
                    for name, path in PROBE_PATHS.items()), "Web readiness, startup or liveness path differs")
    require(type(service.get("termination_grace_seconds")) is int
            and service["termination_grace_seconds"] == 1050, "Web termination grace differs")
    pre_stop = service.get("pre_stop")
    command = pre_stop.get("exec") if isinstance(pre_stop, dict) else None
    files = service.get("files")
    require(isinstance(files, list) and all(isinstance(file, dict) for file in files),
            "Effective Web file mounts are missing")
    configs = [file.get("content") for file in files if file.get("path") == NGINX_PATH]
    require(len(configs) == 1 and isinstance(configs[0], str)
            and sha256(configs[0].encode()).hexdigest() == nginx_sha256,
            "Effective Nginx mount differs from the reviewed digest")
    hooks = [file.get("content") for file in files if file.get("path") == HOOK_PATH]
    if hooks:
        require(hooks == [HOOK_SOURCE.read_text(encoding="utf-8")] and command == ["/bin/sh", HOOK_PATH],
                "Mounted Web preStop differs from the reviewed hook")
    else:
        require(command == [HOOK_PATH], "Web preStop executable is missing from the effective service")


def read(origin, path, *, evidence=None):
    target = urlsplit(origin)
    loopback_http = target.scheme == "http" and target.hostname in ("127.0.0.1", "::1", "localhost")
    require(target.hostname and target.path in ("", "/") and target.username is None and target.password is None
            and not target.query and not target.fragment and (target.scheme == "https" or loopback_http),
            "Web preflight needs an HTTPS origin or a loopback probe")
    transport = http.client.HTTPSConnection if target.scheme == "https" else http.client.HTTPConnection
    connection = transport(target.hostname, target.port, timeout=10)
    marker = uuid4().hex
    observed = {"path": path, "marker": marker, "started_at": datetime.now(timezone.utc).isoformat()}
    if evidence is not None:
        evidence.append(observed)
    try:
        connection.request("GET", f"{path}?cma_web_preflight={marker}",
                           headers={"Cache-Control": "no-cache", "Accept-Encoding": "identity"})
        reply = connection.getresponse()
        observed["status"] = reply.status
        request_id = reply.headers.get("X-Request-ID")
        if matches(r"[A-Za-z0-9_.:-]{1,128}", request_id):
            observed["request_id"] = request_id
        media_type = reply.headers.get("Content-Type", "").split(";", 1)[0].strip().lower()
        cache = {value.strip().lower() for header in reply.headers.get_all("Cache-Control", [])
                 for value in header.split(",")}
        body = reply.read(MAX_RESPONSE_BYTES + 1)
        require(len(body) <= MAX_RESPONSE_BYTES, "Web response exceeds its read budget")
        observed["body_sha256"] = sha256(body).hexdigest()
        return reply.status, media_type, cache, body
    finally:
        connection.close()


def require_readiness(origin, *, draining, fetch):
    status, media_type, cache, body = fetch(origin, READY_PATH)
    expected = (503, b"draining\n") if draining else (200, b"ready\n")
    require((status, body) == expected and media_type == "text/plain" and "no-store" in cache,
            "Web readiness response differs from the expected drain state")


def require_asset(origin, sentinel, *, fetch):
    status, media_type, cache, body = fetch(origin, sentinel["path"])
    require(status == 200 and media_type in ("text/javascript", "application/javascript")
            and {"public", "immutable"} <= cache and not {"no-store", "private"} & cache
            and sha256(body).hexdigest() == sentinel["sha256"], "Web asset differs from the selected release")


def require_identity(origin, version, source, *, fetch):
    status, media_type, _, body = fetch(origin, "/build-info.json")
    require(status == 200 and media_type == "application/json", "Web build identity is unavailable")
    info = document(body.decode("utf-8"))
    require(isinstance(info, dict) and info.get("version") == version and info.get("revision") == source
            and info.get("local") is False, "Web build identity differs from the selected release")


def verify(service, image, nginx_sha256, version, source, sentinel, edge, draining, *, fetch=read):
    validate_service(service, image, nginx_sha256)
    version_tuple(version)
    require(matches(r"[0-9a-f]{40}", source) and isinstance(sentinel, dict)
            and matches(ASSET, sentinel.get("path")) and sentinel["path"].endswith(".js")
            and all(matches(HASH, sentinel.get(key)) for key in ("sha256", "html_sha256")),
            "Web release identity or sentinel is malformed")
    require(edge.rstrip("/") != draining.rstrip("/"), "The drain witness must use a distinct direct origin")
    require_readiness(edge, draining=False, fetch=fetch)
    require_identity(edge, version, source, fetch=fetch)
    status, media_type, cache, body = fetch(edge, "/")
    require(status == 200 and media_type == "text/html" and "no-store" in cache
            and sha256(body).hexdigest() == sentinel["html_sha256"], "Web HTML differs through the edge")
    require_asset(edge, sentinel, fetch=fetch)
    require_readiness(draining, draining=True, fetch=fetch)
    require_identity(draining, version, source, fetch=fetch)
    require_asset(draining, sentinel, fetch=fetch)


def read_json(path):
    require(path.is_file() and 0 < path.stat().st_size <= MAX_INPUT_BYTES, "Web preflight input is missing or oversized")
    return document(path.read_text(encoding="utf-8"))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--service", type=Path, required=True, help="Effective Web service JSON from the controller")
    parser.add_argument("--image", required=True, help="Selected digest-pinned Web image")
    parser.add_argument("--nginx-sha256", required=True, help="SHA-256 of the reviewed Nginx mount")
    parser.add_argument("--version", required=True)
    parser.add_argument("--source", required=True)
    parser.add_argument("--sentinel", type=Path, required=True, help="Selected release sentinel from the prepare receipt")
    parser.add_argument("--edge-origin", required=True, help="External route under pre-exposure qualification")
    parser.add_argument("--draining-origin", required=True, help="Direct origin of an isolated replica running the selected image and hook")
    parser.add_argument("--evidence", type=Path, help="New file for the first-attempt HTTP evidence")
    args = parser.parse_args()
    requests = {"edge": [], "draining": []}

    def fetch(origin, path):
        role = "edge" if origin == args.edge_origin else "draining"
        return read(origin, path, evidence=requests[role])

    try:
        verify(read_json(args.service), args.image, args.nginx_sha256, args.version, args.source,
               read_json(args.sentinel), args.edge_origin, args.draining_origin, fetch=fetch)
    except Refusal as error:
        parser.exit(1, f"Web rollout preflight refused at {error.site}: {error}\n")
    except (OSError, ValueError, http.client.HTTPException):
        parser.exit(1, "Web rollout preflight could not read its inputs or endpoints\n")
    finally:
        if args.evidence:
            try:
                with args.evidence.open("x", encoding="utf-8") as output:
                    json.dump({"image": args.image, "version": args.version, "source": args.source,
                               "nginx_sha256": args.nginx_sha256, "requests": requests}, output, indent=2)
                    output.write("\n")
            except OSError:
                parser.exit(1, "Web rollout preflight could not write a new evidence file\n")
    print(f"PASS Web rollout preflight {args.version} {args.source} {args.image}")


if __name__ == "__main__":
    main()
