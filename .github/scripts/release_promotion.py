"""Record a release by promoting staging-verified images; a release never rebuilds.

A release reaches main as a merge commit M whose second parent R is the
develop commit the release PR qualified. For every published component
(Agent Manager, Event Gateway, web and both runtimes) the release records the
image production promotion would choose: the newest published component
version whose source is R or an ancestor of R on develop's first-parent
history. That exact image must have been deployed and controller-verified by
staging, as prod_cd_select requires, so each image is built and scanned once,
by the reviewed staging publication gate.

``select`` writes the selected images as one canonical JSON output.
``promote`` assigns the release tags to the selected digests with a
digest-preserving copy, reuses a tag that already names its digest and
refuses one that names anything else. Every image's short-SHA tag is assigned
before any version tag: the version tag is what a deployment names, and a
release that fails before it leaves no version claimed.
"""

import hashlib
import json
import os
import subprocess
import sys
from pathlib import Path

from staging_cd_common import matches, require
from staging_cd_versions import SEMVER

# Release matrix names of the selection targets.
PUBLISHED = {"am": "api", "eg": "cmaeg", "web": "web", "runtime": "runtime", "runtime-omarchy": "runtime-omarchy"}
OWNER = "ghcr.io/chronoaiproject/"
DIGEST = r"sha256:[0-9a-f]{64}"
SKOPEO_SECONDS = 600


def select(released, api, registry, download, deployables):
    """Return {release component: image} for release commit R, or refuse."""
    import prod_cd_select as selection
    from staging_cd_receipts import validate_receipt

    components = {target: deployables[target] for target in sorted(PUBLISHED)}
    versions = selection.select_versions(released, components, api.tags, api.history())
    receipts = {}
    for target, version in versions.items():
        name, published = components[target] + "/v" + version, api.tags(target)
        sources = [tag["target"] for tag in published if tag["name"].startswith(components[target] + "/v")]
        require(len(set(sources)) == len(sources), "Published component sources are duplicated")
        tags = [tag for tag in published if tag["name"] == name]
        require(len(tags) == 1, "A selected component version is absent or ambiguous")
        receipt = validate_receipt(tags[0]["receipt"])
        require(receipt["tag"] == name and receipt["source"] == tags[0]["target"],
                "Published version target differs")
        require(registry.exists(receipt["repository"], receipt["digest"]) is True,
                "Published digest is not publicly available")
        manifest = registry.manifest(receipt["repository"], receipt["identity"]["version"])
        require(manifest is not None and manifest[0] == receipt["digest"],
                "Published version manifest differs from the immutable receipt")
        receipts[target] = receipt
    images = {target: r["repository"] + "@" + r["digest"] for target, r in receipts.items()}
    missing = sorted(set(images) - selection.staging_verified(images, api.request, download))
    require(not missing, "Staging never verified a released image")
    return {PUBLISHED[target]: {"repository": r["repository"], "digest": r["digest"],
                                "version": r["identity"]["version"], "source": r["source"]}
            for target, r in receipts.items()}


def image_name(component):
    from deployables import name

    return name(component)


def selected_images(images):
    """Validate the selection: exactly one well-formed image per release component."""
    value = json.loads(images)
    require(isinstance(value, dict) and set(value) == set(PUBLISHED.values()), "Release image selection is malformed")
    for component, image in value.items():
        require(isinstance(image, dict) and set(image) == {"repository", "digest", "version", "source"}
                and image["repository"] == OWNER + image_name(component)
                and matches(DIGEST, image["digest"]) and matches(SEMVER, image["version"])
                and matches(r"[0-9a-f]{40}", image["source"]), "Release image selection is malformed")
    return value


class Skopeo:
    """The pinned Skopeo wrapper, reading the registry credential from its authfile."""

    def __init__(self, command, authfile, runner=subprocess.run):
        self.command, self.authfile, self.runner = command, authfile, runner

    def run(self, *args):
        result = self.runner([self.command, *args], capture_output=True, timeout=SKOPEO_SECONDS, check=False)
        require(result.returncode == 0, "The registry operation failed")
        return result.stdout

    def tags(self, repository):
        listing = json.loads(self.run("list-tags", "--authfile", self.authfile, "docker://" + repository))
        require(isinstance(listing, dict) and isinstance(listing.get("Tags"), list)
                and all(isinstance(tag, str) for tag in listing["Tags"]), "The registry tag listing is malformed")
        return set(listing["Tags"])

    def digest(self, repository, tag):
        raw = self.run("inspect", "--raw", "--authfile", self.authfile, f"docker://{repository}:{tag}")
        return "sha256:" + hashlib.sha256(raw).hexdigest()

    def copy(self, repository, digest, tag):
        self.run("copy", "--all", "--preserve-digests", "--authfile", self.authfile,
                 f"docker://{repository}@{digest}", f"docker://{repository}:{tag}")


def promote_all(images, version, release, skopeo):
    """Assign every short-SHA tag, then every version tag; return the tags assigned."""
    require(matches("v" + SEMVER, version) and matches(r"[0-9a-f]{40}", release), "Release identity is invalid")
    assigned = []
    for tag in (release[:7], version):
        for component in sorted(images):
            if promote(images[component], tag, skopeo):
                assigned.append((component, tag))
    return assigned


def promote(image, tag, skopeo):
    """Assign one release tag to the selected digest; return whether this call assigned it."""
    repository, digest = image["repository"], image["digest"]
    if tag in skopeo.tags(repository):
        require(skopeo.digest(repository, tag) == digest, "A release tag already names a different image")
        return False
    skopeo.copy(repository, digest, tag)
    require(skopeo.digest(repository, tag) == digest, "The release tag does not name the promoted image")
    return True


def main(argv):
    from staging_cd_resolve import output
    from staging_cd_runtime import run_private

    env = os.environ

    def operation(_masker):
        if argv == ["select"]:
            from prod_cd_select import gh_download, release_commit
            from staging_cd_github import GitHub
            from staging_cd_receipts import canonical
            from staging_cd_registry import Registry
            from staging_cd_versions import DEPLOYABLES

            released = release_commit(env["GITHUB_SHA"], Path.cwd())
            images = select(released, GitHub(env.get("GH_TOKEN")), Registry(),
                            gh_download(env["GH_TOKEN"]), DEPLOYABLES)
            output(env, "released", released)
            output(env, "images", canonical(images))
            return True
        require(argv == ["promote"], "Release promotion usage is invalid")
        promote_all(selected_images(env["RELEASE_IMAGES"]), env["RELEASE_VERSION"], env["GITHUB_SHA"],
                    Skopeo(env["SKOPEO_COMMAND"], env["SKOPEO_AUTHFILE"]))
        return True

    return run_private(operation, None, subject="release-promotion")


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
