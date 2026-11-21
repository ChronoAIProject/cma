"""Publication inputs that select images from outside their build contexts."""

# Only the AM/runtime contract evidence qualifies. Every group receipt binds
# versions derived from these two files, and a contract transition must
# republish the complete group, so they select each group image even where its
# build context omits them. They mirror staging_cd_compatibility's VERSION_FILE,
# RECIPE and GROUP; a staging regression keeps the copies equal.
CONTRACT_FILES = frozenset({
    'api/crates/layer3-adapter/runtime-versions.json',
    'api/crates/layer4-runtime/Dockerfile',
})
CONTRACT_IMAGES = frozenset({'cma-agent-manager', 'cma-runtime', 'cma-runtime-omarchy'})


def contract_images(paths):
    # Publisher, scanner and gate code is not an image input. A lane whose
    # latest final contains every input change stays current; staging
    # allocation retries a lane whose latest attempt did not finalize.
    return CONTRACT_IMAGES if CONTRACT_FILES & set(paths) else frozenset()
