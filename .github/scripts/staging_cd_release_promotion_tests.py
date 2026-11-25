"""Release promotion: staging-verified selection of every image and digest-preserving tags."""

import hashlib
import json
import unittest
from unittest.mock import Mock, patch

import release_promotion as promotion
from staging_cd_common import Refusal
from staging_cd_targets import TARGETS
from staging_cd_versions import DEPLOYABLES

RELEASED, OLDER, NEWER = "2" * 40, "3" * 40, "4" * 40
MERGE = "1" * 40
VERSION = "v1.4.0"


class History:
    def __init__(self, order):
        self.positions = {sha: -index for index, sha in enumerate(order)}

    def position(self, sha):
        if sha not in self.positions:
            raise Refusal("Release source is not on verified develop first-parent history")
        return self.positions[sha]


def digest_of(text):
    return "sha256:" + hashlib.sha256(text.encode()).hexdigest()


def receipt(target, version, source):
    name = DEPLOYABLES[target]
    return {"tag": f"{name}/v{version}", "source": source, "repository": promotion.OWNER + name,
            "digest": digest_of(target + version), "identity": {"component": target, "version": version}}


def published(target, *rows):
    return [{"name": f"{DEPLOYABLES[target]}/v{version}", "target": source, "receipt": receipt(target, version, source)}
            for version, source in rows]


class Registry:
    def __init__(self, missing=()):
        self.missing = set(missing)

    def exists(self, repository, digest):
        return digest not in self.missing

    def manifest(self, repository, reference):
        target = next(t for t in TARGETS if repository.endswith("/" + DEPLOYABLES[t]))
        return (digest_of(target + reference),)


class SelectionTests(unittest.TestCase):
    def setUp(self):
        self.tags = {target: published(target, ("1.0.0", OLDER), ("1.0.1", RELEASED), ("1.0.2", NEWER))
                     for target in TARGETS}
        self.api = Mock(tags=lambda target: self.tags[target], request=Mock(),
                        history=Mock(return_value=History([NEWER, RELEASED, OLDER])))
        self.receipts = patch("staging_cd_receipts.validate_receipt", side_effect=lambda value: value)
        self.receipts.start()
        self.addCleanup(self.receipts.stop)

    def verified(self, components):
        return patch("prod_cd_select.staging_verified", return_value=set(components))

    def test_every_published_image_is_chosen_at_or_before_the_release_commit(self):
        self.assertEqual(set(promotion.PUBLISHED), set(TARGETS))
        with self.verified(TARGETS) as verified:
            images = promotion.select(RELEASED, self.api, Registry(), Mock(), DEPLOYABLES)
        self.assertEqual(set(images), set(promotion.PUBLISHED.values()))
        for target, component in promotion.PUBLISHED.items():
            self.assertEqual(images[component], {"repository": promotion.OWNER + DEPLOYABLES[target],
                                                 "digest": digest_of(target + "1.0.1"),
                                                 "version": "1.0.1", "source": RELEASED})
        checked = verified.call_args.args[0]
        self.assertEqual(checked["am"], promotion.OWNER + DEPLOYABLES["am"] + "@" + digest_of("am1.0.1"))

    def test_an_image_staging_never_verified_refuses_the_whole_release(self):
        with self.verified(set(TARGETS) - {"runtime-omarchy"}), \
                self.assertRaisesRegex(Refusal, "never verified"):
            promotion.select(RELEASED, self.api, Registry(), Mock(), DEPLOYABLES)

    def test_a_missing_digest_or_moved_version_tag_is_refused(self):
        with self.verified(TARGETS), self.assertRaisesRegex(Refusal, "not publicly available"):
            promotion.select(RELEASED, self.api, Registry(missing={digest_of("web1.0.1")}), Mock(), DEPLOYABLES)
        moved = Registry()
        moved.manifest = lambda repository, reference: ("sha256:" + "0" * 64,)
        with self.verified(TARGETS), self.assertRaisesRegex(Refusal, "manifest differs"):
            promotion.select(RELEASED, self.api, moved, Mock(), DEPLOYABLES)

    def test_a_receipt_for_another_source_is_refused(self):
        self.tags["eg"][1]["receipt"]["source"] = OLDER
        with self.verified(TARGETS), self.assertRaises(Refusal):
            promotion.select(RELEASED, self.api, Registry(), Mock(), DEPLOYABLES)

    def test_two_versions_of_one_component_from_the_same_source_are_refused(self):
        self.tags["web"][0]["target"] = RELEASED
        with self.verified(TARGETS), self.assertRaisesRegex(Refusal, "duplicated"):
            promotion.select(RELEASED, self.api, Registry(), Mock(), DEPLOYABLES)


def selection(**changes):
    value = {component: {"repository": promotion.OWNER + promotion.image_name(component),
                         "digest": "sha256:" + "5" * 64, "version": "0.2.3", "source": RELEASED}
             for component in promotion.PUBLISHED.values()}
    value["web"].update(changes)
    return json.dumps(value)


class SelectedImageTests(unittest.TestCase):
    def test_only_a_complete_well_formed_selection_is_accepted(self):
        images = promotion.selected_images(selection())
        self.assertEqual(images["web"]["repository"], promotion.OWNER + "cma-web")
        for broken in (selection(repository=promotion.OWNER + "cma-agent-manager"),
                       selection(digest="sha256:abc"), selection(version="v0.2.3"), selection(source="main")):
            with self.assertRaises(Refusal):
                promotion.selected_images(broken)
        partial = json.loads(selection())
        del partial["runtime-omarchy"]
        with self.assertRaises(Refusal):
            promotion.selected_images(json.dumps(partial))
        extra = json.loads(selection())
        extra["image-manager"] = extra["web"]
        with self.assertRaises(Refusal):
            promotion.selected_images(json.dumps(extra))


class Registry2:
    """A fake registry behind the Skopeo command line; ``tags`` belong to the web repository."""

    def __init__(self, tags):
        self.references = {("ghcr.io/chronoaiproject/cma-web", tag): value for tag, value in tags.items()}
        self.calls = []

    @property
    def tags(self):
        return {tag: value for (repository, tag), value in self.references.items()
                if repository == "ghcr.io/chronoaiproject/cma-web"}

    def __call__(self, args, **_kwargs):
        self.calls.append(args[1:])
        verb = args[1]
        if verb == "list-tags":
            repository = args[-1][len("docker://"):]
            listed = sorted(tag for (name, tag) in self.references if name == repository)
            return Mock(returncode=0, stdout=json.dumps({"Tags": listed}).encode())
        if verb == "inspect":
            repository, tag = args[-1][len("docker://"):].rsplit(":", 1)
            return Mock(returncode=0, stdout=self.references[(repository, tag)].encode())
        if verb == "copy":
            source, destination = args[-2], args[-1]
            repository, tag = destination[len("docker://"):].rsplit(":", 1)
            self.references[(repository, tag)] = MANIFEST if source.endswith(DIGEST) else "other"
            return Mock(returncode=0, stdout=b"")
        return Mock(returncode=1, stdout=b"")


MANIFEST = '{"schemaVersion":2}'
DIGEST = digest_of(MANIFEST)
IMAGE = {"repository": promotion.OWNER + "cma-web", "digest": DIGEST, "version": "0.2.3", "source": RELEASED}


class PromotionTests(unittest.TestCase):
    def skopeo(self, registry):
        return promotion.Skopeo("skopeo", "/auth/config.json", runner=registry)

    def test_every_short_sha_tag_precedes_any_version_tag(self):
        images = {"api": dict(IMAGE, repository=promotion.OWNER + "cma-agent-manager"), "web": IMAGE}
        registry = Registry2({})
        self.assertEqual(promotion.promote_all(images, VERSION, MERGE, self.skopeo(registry)),
                         [("api", MERGE[:7]), ("web", MERGE[:7]), ("api", VERSION), ("web", VERSION)])
        self.assertFalse(promotion.promote_all(images, VERSION, MERGE, self.skopeo(registry)))

    def test_a_refused_short_sha_tag_claims_no_version(self):
        images = {"api": dict(IMAGE, repository=promotion.OWNER + "cma-agent-manager"), "web": IMAGE}
        registry = Registry2({MERGE[:7]: "other"})
        with self.assertRaisesRegex(Refusal, "different image"):
            promotion.promote_all(images, VERSION, MERGE, self.skopeo(registry))
        self.assertNotIn(VERSION, registry.tags)

    def test_invalid_release_identity_is_refused(self):
        for version, release in (("1.4.0", MERGE), (VERSION, "main")):
            with self.assertRaises(Refusal):
                promotion.promote_all({"web": IMAGE}, version, release, self.skopeo(Registry2({})))

    def test_an_absent_tag_is_assigned_by_digest_preserving_copy(self):
        registry = Registry2({"0.2.3": MANIFEST})
        self.assertTrue(promotion.promote(IMAGE, VERSION, self.skopeo(registry)))
        copies = [call for call in registry.calls if call[0] == "copy"]
        self.assertEqual(copies, [["copy", "--all", "--preserve-digests", "--authfile", "/auth/config.json",
                                   f"docker://{IMAGE['repository']}@{DIGEST}", f"docker://{IMAGE['repository']}:{VERSION}"]])
        self.assertEqual(registry.tags[VERSION], MANIFEST)

    def test_a_rerun_reuses_a_tag_that_already_names_the_promoted_digest(self):
        registry = Registry2({VERSION: MANIFEST})
        self.assertFalse(promotion.promote(IMAGE, VERSION, self.skopeo(registry)))
        self.assertFalse([call for call in registry.calls if call[0] == "copy"])

    def test_a_tag_naming_another_image_is_refused_without_copying(self):
        registry = Registry2({MERGE[:7]: "other"})
        with self.assertRaisesRegex(Refusal, "different image"):
            promotion.promote(IMAGE, MERGE[:7], self.skopeo(registry))
        self.assertFalse([call for call in registry.calls if call[0] == "copy"])

    def test_a_copy_that_does_not_land_the_digest_is_refused(self):
        registry = Registry2({})
        with self.assertRaisesRegex(Refusal, "does not name"):
            promotion.promote(dict(IMAGE, digest="sha256:" + "6" * 64), VERSION, self.skopeo(registry))

    def test_registry_failures_are_refused(self):
        failing = promotion.Skopeo("skopeo", "/auth/config.json", runner=lambda *_a, **_k: Mock(returncode=1))
        with self.assertRaisesRegex(Refusal, "registry operation failed"):
            promotion.promote(IMAGE, VERSION, failing)


if __name__ == "__main__":
    unittest.main()
