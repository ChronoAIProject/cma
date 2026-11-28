"""Read an asset catalog from a digest-pinned Web OCI archive."""

from contextlib import ExitStack
import json
from pathlib import Path
import tarfile
import tempfile

from staging_cd_common import require
from staging_cd_web_overlay import (
    ARCHIVE_ENTRIES, ARCHIVE_LIMIT, FALLBACK, PUBLIC_READ_LIMIT, MANIFEST_LABEL,
    catalog_bytes, hashed, load_assets, scanner_types, validate_base,
)


def incumbent_catalog(archive, directory):
    with ExitStack() as lifetime:
        _, assets, _, digest = load_assets(
            archive, directory, lifetime, scanner_types(), ARCHIVE_LIMIT,
        )
    return json.loads(catalog_bytes(assets)), digest


def prepare_catalog(archive, digest, directory):
    reader_type, budget_type, read_path, _ = scanner_types()
    with tempfile.TemporaryDirectory(dir=directory, prefix="web-prepare-catalog-") as temporary:
        with reader_type(archive.archive, Path(temporary), budget_type(ARCHIVE_LIMIT, ARCHIVE_ENTRIES)) as reader:
            kind, images = reader.images()
            image = validate_base(kind, images, archive, allow_asset_overlay=True)
            labels = image["config"]["config"]["Labels"]
            require(labels.get(MANIFEST_LABEL) == digest, "Web prepare catalog differs from its image label")
            path = f"{FALLBACK}cma-asset-manifest-{digest}.json"
            layer = reader.layer(image["layers"][-1], image["diff_ids"][-1])
            payload = None
            with tarfile.open(layer, "r:") as contents:
                for member in contents:
                    reader.budget.entry()
                    if read_path(member.name) != path:
                        continue
                    require(payload is None and member.isfile() and not member.issparse()
                            and 0 < member.size <= PUBLIC_READ_LIMIT, "Web prepare catalog entry is invalid")
                    with contents.extractfile(member) as stream:
                        payload = stream.read()
            require(payload is not None and hashed(payload) == digest, "Web prepare catalog digest differs")
            return json.loads(payload)
