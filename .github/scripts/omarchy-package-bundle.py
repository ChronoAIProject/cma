"""Build the digest-pinned Omarchy package bundle from its committed lock.

The Omarchy runtime stages install every Arch and Omarchy package from this
bundle: a FROM scratch OCI image whose /bundle tree is a file:// pacman
repository per repository, so a cold build needs no live package server.

  lock    regenerate the lock from the snapshot's sync databases and the
          resolved package list of every Omarchy install step
  tag     validate the lock and print the bundle tag for a snapshot
  build   download every lock entry, verify it, write a reproducible OCI layout
  absent  succeed only when a registry reports that a tag does not exist

Every file is pinned by size and sha256. Arch packages must also match their
snapshot database, which binds each package to its detached signature, and
every package signature is checked with gpgv against the keyring package the
lock pins. pacman verifies them again, with its full trust model, when the
image is built. Arch does not sign its sync databases; the lock pins them.
"""

import argparse
import base64
import concurrent.futures
import hashlib
import io
import json
import os
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path, PurePosixPath

SCHEMA = "cma-omarchy-package-lock-v1"
LOCK = Path(__file__).with_name("omarchy-packages.lock.json")
SOURCE = "https://github.com/ChronoAIProject/cma"
ARCHIVE = "https://archive.archlinux.org/repos/{snapshot}/{repo}/os/x86_64/{name}"
# Fallbacks serve the same files; every byte is still checked against the lock.
ARCH_MIRRORS = ("https://stable-mirror.omarchy.org/{repo}/os/x86_64/{name}",)
REISSUE_MIRRORS = ("https://geo.mirror.pkgbuild.com/{repo}/os/x86_64/{name}",
                   "https://stable-mirror.omarchy.org/{repo}/os/x86_64/{name}")
OMARCHY = "https://pkgs.omarchy.org/stable/x86_64/{name}"
ARCH_REPOS = ("core", "extra", "multilib")
KEYRINGS = {"arch": ("archlinux.gpg", "archlinux-revoked"), "omarchy": ("omarchy.gpg", "omarchy-revoked")}
# The keyring that signs each repository directory's packages.
REPOSITORY_KEYRINGS = {**{repo: "arch" for repo in ARCH_REPOS}, "omarchy": "omarchy"}
SNAPSHOT = re.compile(r"[0-9]{4}/[0-9]{2}/[0-9]{2}")
HEX = re.compile(r"[0-9a-f]{64}")
NAME = re.compile(r"[A-Za-z0-9@._+:-]+")
LAYER = "application/vnd.oci.image.layer.v1.tar"
REFUSED_STATUS = {"BADSIG", "ERRSIG", "EXPSIG", "EXPKEYSIG", "REVKEYSIG", "NO_PUBKEY", "NODATA"}


class BundleError(Exception):
    pass


class Hosts:
    """Failure counts per host, so a server that keeps failing is tried after the others."""

    def __init__(self):
        self.failures, self.lock = {}, threading.Lock()

    def order(self, urls):
        with self.lock:
            return sorted(urls, key=lambda u: self.failures.get(urllib.parse.urlparse(u).hostname, 0))

    def record(self, url, failed):
        host = urllib.parse.urlparse(url).hostname
        with self.lock:
            self.failures[host] = self.failures.get(host, 0) + 1 if failed else 0


def require(condition, message):
    if not condition:
        raise BundleError(message)


def sha256(path):
    with open(path, "rb") as f:
        return hashlib.file_digest(f, "sha256").hexdigest()


def canonical(lock):
    """The only accepted rendering: one file entry per line, sorted keys."""
    head = {k: v for k, v in lock.items() if k != "files"}
    lines = [json.dumps(entry, sort_keys=True, separators=(",", ":")) for entry in lock["files"]]
    body = json.dumps(head, sort_keys=True, indent=1)[:-2]
    return body + ',\n "files": [\n  ' + ",\n  ".join(lines) + "\n ]\n}\n"


def safe_path(path):
    parts = PurePosixPath(path).parts
    return (bool(parts) and not path.startswith("/") and ".." not in parts and "." not in parts
            and all(NAME.fullmatch(p) for p in parts) and str(PurePosixPath(path)) == path)


def validate(lock, raw=None):
    require(isinstance(lock, dict) and lock.get("schema") == SCHEMA, "lock schema is not supported")
    require(set(lock) == {"schema", "snapshot", "keyrings", "files"}, "lock has unexpected fields")
    require(isinstance(lock["snapshot"], str) and SNAPSHOT.fullmatch(lock["snapshot"]), "lock snapshot is invalid")
    files = lock["files"]
    require(isinstance(files, list) and files, "lock lists no files")
    paths = [e.get("path") for e in files if isinstance(e, dict)]
    require(len(paths) == len(files) and paths == sorted(set(paths)), "lock paths must be unique and sorted")
    by_path = {e["path"]: e for e in files}
    for e in files:
        kind = e.get("kind")
        common = {"path", "kind", "size", "sha256", "urls"}
        extra = {"package": {"signature", "keyring"} | ({"database"} if "database" in e else set())}.get(kind, set())
        require(kind in {"database", "package", "signature"} and set(e) == common | extra, "lock entry has unexpected fields")
        require(safe_path(e["path"]), "lock path is not a safe relative path")
        require(isinstance(e["size"], int) and e["size"] > 0 and isinstance(e["sha256"], str)
                and HEX.fullmatch(e["sha256"]), "lock entry needs a size and sha256")
        require(isinstance(e["urls"], list) and e["urls"] and len(set(e["urls"])) == len(e["urls"])
                and all(isinstance(u, str) and u.startswith("https://") and u.rsplit("/", 1)[1] == e["path"].rsplit("/", 1)[1]
                        for u in e["urls"]), "lock urls must be https and name the same file")
        if kind == "package":
            sig = by_path.get(e["signature"])
            require(sig is not None and sig["kind"] == "signature" and e["signature"] == e["path"] + ".sig",
                    "package signature entry is missing")
            require(e["keyring"] in KEYRINGS, "package keyring is unknown")
            repository = PurePosixPath(e["path"]).parts[0]
            require(REPOSITORY_KEYRINGS.get(repository) == e["keyring"], "package keyring does not sign its repository")
            require("database" in e or repository not in ARCH_REPOS, "Arch package is not bound to its snapshot database")
            if "database" in e:
                db = by_path.get(e["database"])
                require(db is not None and db["kind"] == "database"
                        and PurePosixPath(e["database"]).parent == PurePosixPath(e["path"]).parent,
                        "package database entry is missing")
        if kind == "signature":
            pkg = by_path.get(e["path"][:-4])
            require(e["path"].endswith(".sig") and pkg is not None and pkg["kind"] == "package", "signature has no package")
    keyrings = lock["keyrings"]
    require(isinstance(keyrings, dict) and set(keyrings) == set(KEYRINGS), "lock keyrings are incomplete")
    for name, path in keyrings.items():
        require(path in by_path and by_path[path]["kind"] == "package" and by_path[path]["keyring"] == name,
                "keyring package is not a locked package of its own keyring")
    if raw is not None:
        require(raw == canonical(lock).encode(), "lock is not in canonical form")
    return by_path


def load(path=LOCK):
    raw = Path(path).read_bytes()
    try:
        lock = json.loads(raw)
    except ValueError as error:
        raise BundleError("lock is not valid JSON") from error
    validate(lock, raw)
    return lock, raw


def tag(lock, raw, snapshot):
    require(snapshot == lock["snapshot"], "requested snapshot does not match the lock")
    return snapshot.replace("/", "-") + "-" + hashlib.sha256(raw).hexdigest()[:12]


def read_database(path):
    """Map package file names to (sha256, signature sha256) from a pacman sync database."""
    entries = {}
    with tarfile.open(path, "r:*") as t:
        for member in t:
            if not (member.isfile() and member.name.endswith("/desc")):
                continue
            fields, key = {}, None
            for line in t.extractfile(member).read().decode().split("\n"):
                if line.startswith("%") and line.endswith("%"):
                    key = line.strip("%")
                    fields[key] = []
                elif line and key:
                    fields[key].append(line)
            name = fields["FILENAME"][0]
            signature = fields.get("PGPSIG", [""])[0]
            raw = base64.b64decode(signature) if signature else b""
            entries[name] = {"name": fields["NAME"][0], "version": fields["VERSION"][0], "sha256": fields["SHA256SUM"][0],
                             "size": int(fields["CSIZE"][0]),
                             "signature": hashlib.sha256(raw).hexdigest() if raw else None, "signature_size": len(raw)}
    return entries


def fetch(urls, target, size, digest, attempts=10, delay=5, runner=subprocess.run, sleep=time.sleep, hosts=None,
          stop=None):
    """Download into target, resuming a partial file only from the URL that wrote it, and fail closed on any mismatch."""
    target = Path(target)
    hosts = hosts or Hosts()
    # One partial file per URL, so one host's bytes are never continued from another host.
    parts = {url: target.with_name(f"{target.name}.{hashlib.sha256(url.encode()).hexdigest()[:12]}.part") for url in urls}
    failure = "no attempt"
    for attempt in range(attempts):
        for url in hosts.order(urls):
            if stop is not None and stop.is_set():
                raise BundleError(f"{target.name}: stopped because another download failed")
            part = parts[url]
            if part.exists() and part.stat().st_size > size:
                part.unlink()
            if not (part.exists() and part.stat().st_size == size):
                # Each curl run resumes from this URL's partial file.
                # curl's own --retry would truncate back to its start offset.
                arguments = ["curl", "--fail", "--location", "--silent", "--show-error", "--connect-timeout", "20",
                             "--speed-limit", "1024", "--speed-time", "30", "--retry", "0", "--continue-at", "-",
                             "--output", str(part)]
                if url.startswith("https://"):
                    arguments += ["--proto", "=https", "--proto-redir", "=https"]
                result = runner(arguments + [url], capture_output=True, text=True, check=False)
                if result.returncode != 0:
                    hosts.record(url, True)
                    failure = f"{url}: curl exit {result.returncode}"
                    if result.returncode in (33, 36):  # the server cannot resume this partial file
                        part.unlink(missing_ok=True)
                    continue
            if part.exists() and part.stat().st_size == size and sha256(part) == digest:
                hosts.record(url, False)
                part.replace(target)
                for other in parts.values():
                    other.unlink(missing_ok=True)
                return attempt
            # The host completed a download of other bytes. That is a failure of
            # this host, and the next try of this URL starts the file afresh.
            hosts.record(url, True)
            failure = f"{url}: content does not match the lock"
            part.unlink(missing_ok=True)
        if attempt + 1 < attempts:
            sleep(delay)
    raise BundleError(f"{target.name}: {failure}")


def dearmor(data):
    """Binary OpenPGP packets from an ASCII-armored keyring; gpgv reads only binary keyrings."""
    if not data.lstrip().startswith(b"-----BEGIN PGP PUBLIC KEY BLOCK-----"):
        return data
    packets, body = [], None
    for line in data.decode("ascii").splitlines():
        line = line.strip()
        if line == "-----BEGIN PGP PUBLIC KEY BLOCK-----":
            body, headers = [], True
        elif line == "-----END PGP PUBLIC KEY BLOCK-----":
            require(body is not None and not headers, "keyring armor is malformed")
            packets.append(base64.b64decode("".join(body), validate=True))
            body = None
        elif body is not None:
            if headers:  # armor headers end at the first blank line
                headers = bool(line)
            elif not line.startswith("="):  # the =XXXX line is the armor checksum
                body.append(line)
    require(packets and body is None, "keyring armor is malformed")
    return b"".join(packets)


def extract_keyring(package, names, directory):
    """Extract usr/share/pacman/keyrings members of a zstd package without a zstd module."""
    stream = subprocess.run(["zstd", "-dcq", str(package)], capture_output=True, check=True).stdout
    found = {}
    with tarfile.open(fileobj=io.BytesIO(stream), mode="r:") as t:
        for member in t:
            base = PurePosixPath(member.name)
            if member.isfile() and str(base.parent) == "usr/share/pacman/keyrings" and base.name in names:
                found[base.name] = Path(directory) / base.name
                data = t.extractfile(member).read()
                found[base.name].write_bytes(dearmor(data) if base.name.endswith(".gpg") else data)
    require(set(found) == set(names), f"{package.name} lacks its keyring files")
    return found


def verify_signature(package, signature, keyring, revoked, home):
    result = subprocess.run(["gpgv", "--homedir", str(home), "--keyring", str(keyring), "--status-fd", "1",
                             str(signature), str(package)], capture_output=True, text=True, check=False)
    statuses = [line.split()[1:] for line in result.stdout.splitlines() if line.startswith("[GNUPG:] ")]
    names = {s[0] for s in statuses if s}
    valid = [s for s in statuses if s and s[0] == "VALIDSIG"]
    require(result.returncode == 0 and "GOODSIG" in names and len(valid) == 1 and not names & REFUSED_STATUS,
            f"{package.name}: signature is not a good signature from the pinned keyring")
    require(valid[0][-1] not in revoked, f"{package.name}: signature is from a revoked key")


def verify(lock, root):
    by_path = validate(lock)
    databases = {p: read_database(root / p) for p, e in by_path.items() if e["kind"] == "database"}
    for path, e in by_path.items():
        if e["kind"] != "package" or "database" not in e:
            continue
        indexed = databases[e["database"]].get(PurePosixPath(path).name)
        signature = by_path[e["signature"]]
        require(indexed is not None and indexed["sha256"] == e["sha256"] and indexed["size"] == e["size"]
                and indexed["signature"] == signature["sha256"] and indexed["signature_size"] == signature["size"],
                f"{path}: lock entry does not match its snapshot database")
    with tempfile.TemporaryDirectory() as scratch:
        scratch = Path(scratch)
        rings = {}
        for name, (ring, revoked) in KEYRINGS.items():
            (scratch / name).mkdir()
            files = extract_keyring(root / lock["keyrings"][name], (ring, revoked), scratch / name)
            revoked_keys = {line.split(":")[0].strip() for line in files[revoked].read_text().splitlines() if line.strip()}
            rings[name] = (files[ring], revoked_keys)
        home = scratch / "gnupg"
        home.mkdir(mode=0o700)
        for path, e in by_path.items():
            if e["kind"] == "package":
                keyring, revoked = rings[e["keyring"]]
                verify_signature(root / path, root / e["signature"], keyring, revoked, home)


def tar_layer(root, prefix, members, destination):
    """A byte-reproducible uncompressed layer: sorted entries, fixed owner, mode and mtime."""
    with tarfile.open(destination, mode="w", format=tarfile.PAX_FORMAT) as t:
        directories = sorted({str(p) for m in members for p in PurePosixPath(prefix, m).parents if str(p) != "."})
        entries = [(d, None) for d in directories] + [(str(PurePosixPath(prefix, m)), root / m) for m in members]
        for name, source in sorted(entries):
            info = tarfile.TarInfo(name)
            info.uid = info.gid = info.mtime = 0
            info.uname = info.gname = ""
            if source is None:
                info.type, info.mode = tarfile.DIRTYPE, 0o755
                t.addfile(info)
            else:
                info.mode, info.size = 0o644, source.stat().st_size
                with open(source, "rb") as f:
                    t.addfile(info, f)


def blob(layout, data=None, path=None):
    """Store bytes, or move a written file, under its sha256 in the layout."""
    if path is None:
        path = layout / "blobs/sha256/incoming"
        path.write_bytes(data)
    digest = sha256(path)
    size = path.stat().st_size
    path.replace(layout / "blobs/sha256" / digest)
    return {"digest": "sha256:" + digest, "size": size}


def write_layout(lock, raw, root, layout, image_tag):
    """Write an OCI image layout whose /bundle tree pacman can use as file:// repositories."""
    layout = Path(layout)
    (layout / "blobs/sha256").mkdir(parents=True)
    layers, diff_ids = [], []
    with tempfile.TemporaryDirectory() as meta:
        meta = Path(meta)
        (meta / "SNAPSHOT").write_text(lock["snapshot"] + "\n")
        (meta / "omarchy-packages.lock.json").write_bytes(raw)
        groups = {"": (meta, ["SNAPSHOT", "omarchy-packages.lock.json"])}
        for path in sorted(e["path"] for e in lock["files"]):
            groups.setdefault(PurePosixPath(path).parts[0], (root, []))[1].append(path)
        for name in sorted(groups):
            source, members = groups[name]
            incoming = layout / "blobs/sha256/incoming"
            tar_layer(source, "bundle", members, incoming)
            descriptor = blob(layout, path=incoming)
            layers.append(dict(descriptor, mediaType=LAYER))
            diff_ids.append(descriptor["digest"])
    labels = {"org.opencontainers.image.source": SOURCE, "dev.chrono.cma.omarchy-packages.snapshot": lock["snapshot"],
              "dev.chrono.cma.omarchy-packages.lock-sha256": hashlib.sha256(raw).hexdigest()}
    config = {"architecture": "amd64", "os": "linux", "created": "1970-01-01T00:00:00Z", "config": {"Labels": labels},
              "rootfs": {"type": "layers", "diff_ids": diff_ids}}
    config_descriptor = blob(layout, json.dumps(config, sort_keys=True, separators=(",", ":")).encode())
    manifest = {"schemaVersion": 2, "mediaType": "application/vnd.oci.image.manifest.v1+json", "annotations": labels,
                "config": dict(config_descriptor, mediaType="application/vnd.oci.image.config.v1+json"), "layers": layers}
    manifest_descriptor = blob(layout, json.dumps(manifest, sort_keys=True, separators=(",", ":")).encode())
    index = {"schemaVersion": 2, "mediaType": "application/vnd.oci.image.index.v1+json",
             "manifests": [dict(manifest_descriptor, mediaType="application/vnd.oci.image.manifest.v1+json",
                                annotations={"org.opencontainers.image.ref.name": image_tag})]}
    (layout / "index.json").write_text(json.dumps(index, sort_keys=True, separators=(",", ":")))
    (layout / "oci-layout").write_text('{"imageLayoutVersion":"1.0.0"}')
    return manifest_descriptor["digest"], sum(layer["size"] for layer in layers)


def download(entries, root, cache=None, parallel=4, attempts=10, delay=5):
    """Place each lock entry at root/<path>: kept, copied from the cache or fetched, and always size and sha256 checked.

    Returns how many entries were fetched."""
    root = Path(root)
    cache = Path(cache) if cache else None
    jobs = []
    for e in entries:
        target = root / e["path"]
        target.parent.mkdir(parents=True, exist_ok=True)
        cached = cache / e["path"] if cache else None
        if target.exists() and target.stat().st_size == e["size"] and sha256(target) == e["sha256"]:
            continue
        if cached and cached.is_file() and cached.stat().st_size == e["size"] and sha256(cached) == e["sha256"]:
            shutil.copyfile(cached, target)
            continue
        jobs.append(e)
    hosts, stop = Hosts(), threading.Event()
    with concurrent.futures.ThreadPoolExecutor(parallel) as pool:
        futures = [pool.submit(fetch, e["urls"], root / e["path"], e["size"], e["sha256"], attempts, delay,
                               hosts=hosts, stop=stop) for e in jobs]
        try:
            for future in concurrent.futures.as_completed(futures):
                future.result()
        except BaseException:
            # The first failure fails the call: queued downloads never start,
            # and running ones stop before their next try.
            pool.shutdown(wait=False, cancel_futures=True)
            stop.set()
            raise
    return len(jobs)


def build(args):
    lock, raw = load(args.lock)
    image_tag = tag(lock, raw, args.snapshot)
    root = Path(args.work)
    downloaded = download(lock["files"], root, args.cache, args.parallel, args.attempts, args.delay)
    verify(lock, root)
    digest, size = write_layout(lock, raw, root, args.output, image_tag)
    summary = {"tag": image_tag, "digest": digest, "layers_bytes": size, "files": len(lock["files"]),
               "downloaded": downloaded, "snapshot": lock["snapshot"]}
    print(json.dumps(summary, sort_keys=True))
    if args.summary:
        Path(args.summary).write_text(json.dumps(summary, sort_keys=True) + "\n")


def generate(args):
    snapshot = args.snapshot
    require(SNAPSHOT.fullmatch(snapshot), "snapshot is invalid")
    databases = {}
    files = []
    for spec in args.database:
        repo, path = spec.split("=", 1)
        require(repo in ARCH_REPOS, "unknown Arch repository")
        name = f"{repo}.db"
        databases[repo] = read_database(path)
        files.append({"path": f"{repo}/os/x86_64/{name}", "kind": "database", "size": os.path.getsize(path),
                      "sha256": sha256(path), "urls": [ARCHIVE.format(snapshot=snapshot, repo=repo, name=name)]})
    reissued = {tuple(r.split("/", 1)) for r in args.reissued}
    wanted = set()
    for resolved in args.resolved:
        for line in Path(resolved).read_text().splitlines():
            repo, name, version = line.split()
            if repo in ARCH_REPOS:
                wanted.add((repo, name, version))
    for repo, name, version in sorted(wanted):
        indexed = [(f, e) for f, e in databases[repo].items() if e["name"] == name]
        require(len(indexed) == 1 and indexed[0][1]["version"] == version, f"{repo}/{name} {version} is not indexed")
        filename, e = indexed[0]
        require(e["signature"], f"{repo}/{name} has no indexed signature")
        templates = REISSUE_MIRRORS if (repo, name) in reissued else (ARCHIVE,) + ARCH_MIRRORS
        signature = {"path": f"{repo}/os/x86_64/{filename}.sig", "kind": "signature", "size": e["signature_size"],
                     "sha256": e["signature"], "urls": [t.format(snapshot=snapshot, repo=repo, name=filename + ".sig") for t in templates]}
        files.append({"path": f"{repo}/os/x86_64/{filename}", "kind": "package", "size": e["size"], "sha256": e["sha256"],
                      "urls": [t.format(snapshot=snapshot, repo=repo, name=filename) for t in templates],
                      "signature": signature["path"], "keyring": "arch", "database": f"{repo}/os/x86_64/{repo}.db"})
        files.append(signature)
    for spec in args.omarchy:
        filename, size, digest, sig_size, sig_digest = spec.split(":")
        path = f"omarchy/x86_64/{filename}"
        files.append({"path": path, "kind": "package", "size": int(size), "sha256": digest, "signature": path + ".sig",
                      "keyring": "omarchy", "urls": [OMARCHY.format(name=filename)]})
        files.append({"path": path + ".sig", "kind": "signature", "size": int(sig_size), "sha256": sig_digest,
                      "urls": [OMARCHY.format(name=filename + ".sig")]})
    keyrings = {"arch": next(e["path"] for e in files if e["kind"] == "package" and "/archlinux-keyring-" in e["path"]),
                "omarchy": next(e["path"] for e in files if e["kind"] == "package" and "/omarchy-keyring-" in e["path"])}
    lock = {"schema": SCHEMA, "snapshot": snapshot, "keyrings": keyrings, "files": sorted(files, key=lambda e: e["path"])}
    validate(lock)
    Path(args.output).write_text(canonical(lock))


class RefuseRedirects(urllib.request.HTTPRedirectHandler):
    """A redirect is raised as its HTTP error, so credentials never follow it to another URL."""

    def redirect_request(self, *args):
        return None


def absent(args):
    """Succeed only when the registry reports the tag (or its whole repository) as unknown."""
    user, token = os.environ.get("REGISTRY_USER", ""), os.environ.get("REGISTRY_TOKEN", "")
    require(user and token, "registry credentials are not set")
    require(re.fullmatch(r"[a-z0-9]+(?:[._/-][a-z0-9]+)*", args.repository) and re.fullmatch(r"[A-Za-z0-9_][A-Za-z0-9_.-]{0,127}", args.tag),
            "repository or tag is invalid")
    basic = base64.b64encode(f"{user}:{token}".encode()).decode()
    opener = urllib.request.build_opener(RefuseRedirects)
    scope = urllib.parse.urlencode({"scope": f"repository:{args.repository}:pull,push", "service": urllib.parse.urlparse(args.registry).hostname})
    try:
        with opener.open(urllib.request.Request(f"{args.registry}/token?{scope}", headers={"Authorization": "Basic " + basic}), timeout=30) as r:
            bearer = json.load(r)["token"]
    except (urllib.error.URLError, ValueError, KeyError) as error:
        raise BundleError("registry did not issue a token") from error
    accept = ", ".join(["application/vnd.oci.image.index.v1+json", "application/vnd.oci.image.manifest.v1+json",
                        "application/vnd.docker.distribution.manifest.v2+json", "application/vnd.docker.distribution.manifest.list.v2+json"])
    request = urllib.request.Request(f"{args.registry}/v2/{args.repository}/manifests/{args.tag}",
                                     headers={"Authorization": "Bearer " + bearer, "Accept": accept})
    try:
        with opener.open(request, timeout=30):
            raise BundleError(f"tag {args.tag} already exists; it is never overwritten")
    except urllib.error.HTTPError as error:
        try:
            codes = {e.get("code") for e in json.loads(error.read() or b"{}").get("errors", [])}
        except ValueError:
            codes = set()
        require(error.code == 404 and codes and codes <= {"MANIFEST_UNKNOWN", "NAME_UNKNOWN", "NOT_FOUND"},
                f"registry answered {error.code} for the tag; cannot prove it is absent")
    except urllib.error.URLError as error:
        raise BundleError("registry is unreachable; cannot prove the tag is absent") from error
    print(f"{args.repository}:{args.tag} is absent")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = parser.add_subparsers(dest="command", required=True)
    p = sub.add_parser("tag")
    p.add_argument("--lock", default=LOCK)
    p.add_argument("--snapshot", required=True)
    p = sub.add_parser("build")
    p.add_argument("--lock", default=LOCK)
    p.add_argument("--snapshot", required=True)
    p.add_argument("--work", required=True)
    p.add_argument("--output", required=True)
    p.add_argument("--cache")
    p.add_argument("--summary")
    p.add_argument("--parallel", type=int, default=4)
    p.add_argument("--attempts", type=int, default=10)
    p.add_argument("--delay", type=float, default=5)
    p = sub.add_parser("lock")
    p.add_argument("--snapshot", required=True)
    p.add_argument("--database", action="append", required=True, help="REPO=PATH of the snapshot's sync database")
    p.add_argument("--resolved", action="append", required=True, help="pacman -Sp --print-format '%%r %%n %%v' output")
    p.add_argument("--reissued", action="append", default=[], help="REPO/NAME served by the archive with other bytes")
    p.add_argument("--omarchy", action="append", default=[], help="FILE:SIZE:SHA256:SIG_SIZE:SIG_SHA256")
    p.add_argument("--output", default=LOCK)
    p = sub.add_parser("absent")
    p.add_argument("--registry", default="https://ghcr.io")
    p.add_argument("--repository", required=True)
    p.add_argument("--tag", required=True)
    args = parser.parse_args(argv)
    try:
        if args.command == "tag":
            lock, raw = load(args.lock)
            print(tag(lock, raw, args.snapshot))
        else:
            {"build": build, "lock": generate, "absent": absent}[args.command](args)
    except (BundleError, subprocess.CalledProcessError, OSError) as error:
        print(f"::error::{error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
