"""Shared release inputs that can change whether any current image publishes."""

from pathlib import PurePosixPath

# These families run outside the image source snapshot. A gate or publisher
# repair must create a fresh publication opportunity even if image bytes match.
# Include the complete pinned scanner bundle: its inventory is itself a gate.
PREFIXES = (
    '.github/scripts/image_',
    '.github/scripts/image-content-',
    '.github/scripts/image-secret-scanner/',
    '.github/image-secret-classifications/',
    '.github/scripts/staging_cd_build',
    '.github/scripts/staging_cd_publish',
    '.github/scripts/staging_cd_publication',
)
FILES = frozenset({
    '.github/workflows/staging-release.yml',
    '.github/scripts/install-image-gitleaks.sh',
    '.github/scripts/api-cargo-cache.py',
    '.github/scripts/release_image_publication.py',
    '.github/scripts/staging_cd_artifacts.py',
    '.github/scripts/staging_cd_cache_context.py',
    '.github/scripts/staging_cd_common.py',
    '.github/scripts/staging_cd_finalize.py',
    '.github/scripts/staging_cd_github.py',
    '.github/scripts/staging_cd_receipts.py',
    '.github/scripts/staging_cd_registry.py',
    '.github/scripts/staging_cd_reservations.py',
    '.github/scripts/staging_cd_timing.py',
    '.github/scripts/staging_cd_versions.py',
})


def affects_publication(paths):
    for path in paths:
        # Standalone regression files are not executed by the publisher. Files
        # in the pinned scanner bundle still count through its manifest above.
        name = PurePosixPath(path).name
        if (PurePosixPath(path).parent.as_posix() == '.github/scripts'
                and (name.startswith('test_') or name.endswith(('_tests.py', '-tests.py')))):
            continue
        if path in FILES or path.startswith(PREFIXES):
            return True
    return False
