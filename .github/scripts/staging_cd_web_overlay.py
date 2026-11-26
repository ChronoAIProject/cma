"""Assemble immutable fallback-asset layers on verified Web OCI archives."""

from contextlib import ExitStack
import copy
from dataclasses import dataclass
import gzip
import hashlib
import io
import json
from pathlib import Path
import re
import shutil
import sys
import tarfile
import tempfile

from image_scanner_policy import verify_bundle
from image_secret_gate import checksum, verified_staging_scan
from staging_cd_common import matches, require
from staging_cd_receipts import canonical, sha256
from staging_cd_web_continuity import PROOF_SCHEMA, REPOSITORY, SCHEMA, binding, validate, validate_proof


SCANNER_ROOT = Path(__file__).resolve().parent / "image-secret-scanner"
HTML = "usr/share/nginx/html/index.html"
BUILD_INFO = "usr/share/nginx/html/build-info.json"
CURRENT = "usr/share/nginx/current/assets/"
FALLBACK = "usr/share/nginx/html/assets/"
SOURCE = "https://github.com/ChronoAIProject/cma"
MANIFEST_LABEL = "ai.chrono.cma.web.asset-manifest-sha256"
BASE_LABEL = "ai.chrono.cma.web.asset-base-image"
OCI_MANIFEST = "application/vnd.oci.image.manifest.v1+json"
OCI_CONFIG = "application/vnd.oci.image.config.v1+json"
OCI_LAYER = "application/vnd.oci.image.layer.v1.tar+gzip"
PUBLIC_READ_LIMIT = 2 * 1024**2
ARCHIVE_LIMIT = 2 * 1024**3
ARCHIVE_ENTRIES = 100000
SAFE_NAME = r"[A-Za-z0-9_-][A-Za-z0-9_.-]{0,239}"
ENTRY = re.compile(rb'<script\b[^>]*\bsrc="/assets/([^"]+\.js)"')
MANAGED = (HTML, BUILD_INFO, CURRENT.rstrip("/"), FALLBACK.rstrip("/"))


@dataclass(frozen=True)
class WebArchive:
    archive: Path
    digest: str
    source: str
    version: str
    catalog_sha256: str | None = None
    completed_rollouts: int = 0


def hashed(data):
    return hashlib.sha256(data).hexdigest()


def encoded(value):
    return canonical(value).encode()


def descriptor(data, media_type):
    return {"digest": "sha256:" + hashed(data), "size": len(data), "mediaType": media_type}


def scanner_types():
    verify_bundle(SCANNER_ROOT.parents[2])
    sys.path.insert(0, str(SCANNER_ROOT))
    try:
        from image_secrets.archive import ImageArchive
        from image_secrets.common import Budget, archive_path, json_bytes
    finally:
        sys.path.pop(0)
    return ImageArchive, Budget, archive_path, json_bytes


def validate_base(kind, images, expected):
    require(kind == "oci" and len(images) == 1, "Web base must contain one immutable OCI image")
    [image] = images
    config = image["config"]
    values = config.get("config")
    labels = values.get("Labels") if isinstance(values, dict) else None
    require(isinstance(labels, dict) and image["manifest"] == expected.digest and config.get("os") == "linux"
            and config.get("architecture") == "amd64" and values.get("User") in ("101", "101:101")
            and labels.get("org.opencontainers.image.title") == "CMA Web"
            and labels.get("org.opencontainers.image.source") == SOURCE
            and labels.get("org.opencontainers.image.version") == expected.version
            and labels.get("org.opencontainers.image.revision") == expected.source
            and not {MANIFEST_LABEL, BASE_LABEL} & labels.keys(),
            "Web asset base identity differs from its immutable release")
    return image


def relevant(path):
    return (path in MANAGED or path.startswith(CURRENT)
            or any(item.startswith(path + "/") for item in MANAGED))


def read_current(archive, image, path_reader, metadata_reader, expected, limit):
    content = {}
    metadata = {}
    total = 0
    for name, diff in zip(image["layers"], image["diff_ids"]):
        expanded = archive.layer(name, diff)
        try:
            with tarfile.open(expanded, "r:") as layer:
                for member in layer:
                    archive.budget.entry()
                    path = path_reader(member.name)
                    parent, _, basename = path.rpartition("/")
                    if basename.startswith(".wh."):
                        erased = parent if basename == ".wh..wh..opq" else "/".join(filter(None, (parent, basename[4:])))
                        require(erased and not relevant(erased), "Web asset base overlays an approved path")
                    if not relevant(path):
                        continue
                    if path not in (HTML, BUILD_INFO) and not path.startswith(CURRENT):
                        require(member.isdir(), "Web asset base contains a non-directory asset ancestor")
                        continue
                    require(member.isfile() and not member.issparse(),
                            "Web asset base contains a nonregular approved path")
                    if path in (HTML, BUILD_INFO):
                        require(member.size <= PUBLIC_READ_LIMIT, "Web identity exceeds its public read budget")
                        with layer.extractfile(member) as stream:
                            metadata[path] = stream.read()
                    elif path.startswith(CURRENT):
                        name = path.removeprefix(CURRENT)
                        require(matches(SAFE_NAME, name) and name not in content,
                                "Web asset base contains an unsafe or duplicated current path")
                        total += member.size
                        require(total <= limit, "Web asset base exceeds its approved content budget")
                        with layer.extractfile(member) as stream:
                            content[name] = stream.read()
        finally:
            expanded.unlink()
    require(content and metadata.keys() == {HTML, BUILD_INFO}, "Web base has no complete asset and identity set")
    info = metadata_reader(metadata[BUILD_INFO])
    require(isinstance(info, dict) and info.get("version") == expected.version and info.get("revision") == expected.source
            and info.get("local") is False, "Web document build information differs from its OCI identity")
    entry = ENTRY.search(metadata[HTML])
    preferred = entry[1].decode("ascii", errors="replace") if entry else None
    require(preferred in content, "Web base has no verified entry asset")
    candidates = [name for name, payload in content.items() if name.endswith(".js") and len(payload) <= PUBLIC_READ_LIMIT]
    require(candidates, "Web base has no readable JavaScript sentinel")
    sentinel = preferred if preferred in candidates else min(candidates, key=lambda name: (len(content[name]), name))
    return content, {"path": "/assets/" + sentinel, "sha256": hashed(content[sentinel]),
                     "html_sha256": hashed(metadata[HTML])}


def add_assets(union, assets, total, *, max_files, max_bytes):
    for name, payload in assets.items():
        if name in union:
            require(union[name] == payload, "Web releases disagree on immutable asset bytes")
        else:
            union[name] = payload
            total += len(payload)
            require(len(union) <= max_files and total <= max_bytes, "Web retention union exceeds its approved budget")
    return total


def add_member(archive, name, payload):
    member = tarfile.TarInfo(name)
    member.size = len(payload)
    archive.addfile(member, io.BytesIO(payload))


def catalog_bytes(assets):
    return encoded({name: {"bytes": len(data), "sha256": hashed(data)} for name, data in assets.items()})


def make_layer(assets):
    marker = catalog_bytes(assets)
    inventory = hashed(marker)
    marker_name = "cma-asset-manifest-" + inventory + ".json"
    require(marker_name not in assets and len(marker) <= PUBLIC_READ_LIMIT, "Web retention manifest marker is invalid")
    expected = dict(assets, **{marker_name: marker})
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w", format=tarfile.PAX_FORMAT) as layer:
        for name in sorted(expected):
            add_member(layer, FALLBACK + name, expected[name])
    raw = buffer.getvalue()
    compressed = io.BytesIO()
    with gzip.GzipFile(filename="", mode="wb", fileobj=compressed, mtime=0) as stream:
        stream.write(raw)
    return compressed.getvalue(), "sha256:" + hashed(raw), inventory


def copy_base_layer(source, output, reference):
    name = "blobs/sha256/" + reference["digest"][7:]
    entry = tarfile.TarInfo(name)
    entry.size = reference["size"]
    with source.stream(name) as stream:
        output.addfile(entry, stream)


def pack_overlay(base, image, reader, layer, diff_id, inventory, destination):
    config = copy.deepcopy(image["config"])
    config["rootfs"]["diff_ids"].append(diff_id)
    # OCI history is optional. When the base records it, account for the new filesystem layer.
    if config.get("history"):
        config["history"].append({"created_by": "Assemble CMA Web fallback assets"})
    config["config"]["Labels"].update({
        MANIFEST_LABEL: inventory, BASE_LABEL: REPOSITORY + "@" + image["manifest"],
    })
    config_payload = encoded(config)
    references = [*base["layers"], descriptor(layer, OCI_LAYER)]
    payload = encoded({"schemaVersion": 2, "mediaType": OCI_MANIFEST, "layers": references,
                       "config": descriptor(config_payload, OCI_CONFIG)})
    selected = descriptor(payload, OCI_MANIFEST)
    index = {"schemaVersion": 2, "manifests": [selected | {"platform": {"architecture": "amd64", "os": "linux"}}]}
    with tarfile.open(destination, "x", format=tarfile.USTAR_FORMAT) as result:
        add_member(result, "index.json", encoded(index))
        add_member(result, "oci-layout", encoded({"imageLayoutVersion": "1.0.0"}))
        for raw in (payload, config_payload, layer):
            add_member(result, "blobs/sha256/" + hashed(raw), raw)
        seen = set()
        for reference in base["layers"]:
            if reference["digest"] not in seen:
                copy_base_layer(reader, result, reference)
                seen.add(reference["digest"])
    return {"archive": destination, "digest": selected["digest"], "archive_sha256": checksum(destination)}


def assemble(incumbent, candidate, retained, directory, *, max_files, max_bytes):
    """Build the two images from immutable release bases and the reviewed retention scope."""
    ImageArchive, Budget, read_path, read_json = scanner_types()
    selected = (*retained, incumbent, candidate)
    require(type(max_files) is int and max_files > 0 and type(max_bytes) is int and max_bytes > 0,
            "Web asset retention requires a positive file and byte budget")
    require(len({item.digest for item in selected}) == len(selected)
            and incumbent.source != candidate.source and incumbent.version != candidate.version,
            "Web asset retention release scope is ambiguous")
    total, union, base_images, sentinels, inputs = 0, {}, {}, {}, []
    with ExitStack() as lifetime:
        for expected in selected:
            workspace = Path(lifetime.enter_context(tempfile.TemporaryDirectory(dir=directory, prefix="web-asset-input-")))
            reader = lifetime.enter_context(ImageArchive(expected.archive, workspace, Budget(ARCHIVE_LIMIT, ARCHIVE_ENTRIES)))
            kind, images = reader.images()
            image = validate_base(kind, images, expected)
            assets, sentinel = read_current(reader, image, read_path, read_json, expected, max_bytes)
            catalog_digest = hashed(catalog_bytes(assets))
            require(expected.catalog_sha256 is None or expected.catalog_sha256 == catalog_digest,
                    "Web asset base differs from its finalized catalog")
            total = add_assets(union, assets, total, max_files=max_files, max_bytes=max_bytes)
            inputs.append({"image": REPOSITORY + "@" + expected.digest, "source_sha": expected.source,
                           "version": expected.version, "catalog_sha256": catalog_digest,
                           "completed_rollouts": expected.completed_rollouts})
            if expected in (incumbent, candidate):
                raw = reader.read("blobs/sha256/" + expected.digest[7:])
                base_images[expected] = (read_json(raw), image, reader)
                sentinels[expected] = sentinel
        layer, diff_id, inventory = make_layer(union)
        output = Path(tempfile.mkdtemp(dir=directory, prefix="web-asset-overlays-"))
        try:
            prepare = pack_overlay(*base_images[incumbent], layer, diff_id, inventory, output / "web-prepare.oci.tar")
            promote = pack_overlay(*base_images[candidate], layer, diff_id, inventory, output / "web-promote.oci.tar")
        except BaseException:
            shutil.rmtree(output)
            raise
    return {"prepare": prepare, "promote": promote, "asset_manifest_sha256": inventory,
            "asset_count": len(union), "asset_bytes": total, "layer_digest": "sha256:" + hashed(layer),
            "inputs": sorted(inputs, key=lambda item: item["image"]),
            "sentinels": {"incumbent": sentinels[incumbent], "candidate": sentinels[candidate]}}


def qualify(assembled, incumbent, identity, candidate_digest, preimage_sha256, source):
    """Scan both overlay archives before producing evidence for a Web release annotation."""
    candidate_base = REPOSITORY + "@" + candidate_digest
    overlays, reports, scanner_identities = {}, {}, set()
    for role, base in (("prepare", incumbent["base_image"]), ("promote", candidate_base)):
        output = assembled[role]
        archive = output["archive"]
        report_dir = archive.parent / (role + "-scan")
        archive_digest, scanner_identity = verified_staging_scan(archive, report_dir, component="web", source=source)
        scanner_identities.add(scanner_identity)
        report_bytes = (report_dir / "REPORT.json").read_bytes()
        reports[role] = json.loads(report_bytes)
        require(checksum(archive) == reports[role]["archive_sha256"] == archive_digest == output["archive_sha256"],
                "Qualified Web overlay archive differs from its scan report")
        overlays[role] = {"image": REPOSITORY + "@" + output["digest"], "base_image": base,
                          "layer_digest": assembled["layer_digest"], "archive_sha256": archive_digest,
                          "credential_report_sha256": hashed(report_bytes)}
    require(len(scanner_identities) == 1, "Web overlay scans used different scanner identities")
    continuity = {
        "schema": SCHEMA, "incumbent": incumbent, "candidate_base_image": candidate_base,
        "prepare_image": overlays["prepare"]["image"], "asset_manifest_sha256": assembled["asset_manifest_sha256"],
        "sentinels": assembled["sentinels"],
    }
    proof = {
        "schema": PROOF_SCHEMA, "continuity_sha256": binding(continuity), "retention_preimage_sha256": preimage_sha256,
        "asset_count": assembled["asset_count"], "asset_bytes": assembled["asset_bytes"],
        "inputs": assembled["inputs"], "overlays": overlays,
    }
    continuity["proof_sha256"] = sha256(proof)
    promote = overlays["promote"]
    validate(continuity, identity, promote["image"])
    validate_proof(proof, continuity, identity, promote["image"], promote["archive_sha256"])
    return {"asset_continuity": continuity, "asset_proof": proof, "counts": reports["promote"]["counts"],
            "scanner_identity": scanner_identities.pop()}
