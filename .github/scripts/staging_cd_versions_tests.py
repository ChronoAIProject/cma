"""Merged-label admission, independent semver histories and no-op selection."""
import unittest
from unittest.mock import Mock

from staging_cd_common import Refusal
import staging_cd_versions as versions

REVISION = "a" * 40


class VersionTests(unittest.TestCase):
    def pr(self, level="patch"):
        return {"number": 42, "merged": True, "merged_at": "fixture",
                "merge_commit_sha": REVISION, "base": {"ref": "develop", "repo": {"full_name": versions.REPOSITORY}},
                "labels": [{"name": "version-upgrade:" + level}]}

    def tag(self, deployable, version):
        name = deployable + "/v" + version
        return {"name": name, "annotation_name": name, "type": "tag", "target_type": "commit", "target": "b" * 40}

    def test_major_minor_and_patch_reset_lower_components(self):
        for level, expected in (("major", "2.0.0"), ("minor", "1.3.0"), ("patch", "1.2.10")):
            self.assertEqual(versions.bump("1.2.9", level), expected)
        for invalid in ("01.2.3", "1.02.3", "1.2.03", "v1.2.3", "1.2.3-dev.1", "1.2", "1.2.3\n", None):
            with self.subTest(invalid=invalid), self.assertRaises(Refusal):
                versions.bump(invalid, "patch")
        with self.assertRaises(Refusal):
            versions.bump("1.2.3", "other")

    def test_exact_merged_pr_and_unambiguous_upgrade_level(self):
        pr = self.pr("minor")
        request = Mock(side_effect=[[pr], pr])
        self.assertEqual(versions.merged_pr_level(REVISION, request), (42, "minor"))
        self.assertEqual(request.call_args_list[-1].args, ("pulls/42",))
        for labels in ([], [{"name": "train"}]):
            with self.subTest(labels=labels):
                self.assertEqual(versions.merged_pr_level(REVISION, Mock(side_effect=[[pr], dict(pr, labels=labels)])),
                                 (42, "patch"))
        for labels in ([{"name": "version-upgrade:patch"}, {"name": "version-upgrade:major"}],
                       [{"name": "version-upgrade:patch"}] * 2,
                       [{"name": "version-upgrade:other"}], None, [None], [{"name": False}]):
            with self.subTest(labels=labels), self.assertRaises(Refusal):
                versions.merged_pr_level(REVISION, Mock(side_effect=[[pr], dict(pr, labels=labels)]))
        for changes in ({"merged": False}, {"merge_commit_sha": "b" * 40}, {"base": {"ref": "main"}}, {"number": 41}):
            with self.subTest(changes=changes), self.assertRaises(Refusal):
                versions.merged_pr_level(REVISION, Mock(side_effect=[[pr], dict(pr, **changes)]))
        for rows in ([], [pr, pr], [None], {}, [dict(pr, merged_at=None)], [dict(pr, number=True)]):
            with self.subTest(rows=rows), self.assertRaises(Refusal):
                versions.merged_pr_level(REVISION, Mock(return_value=rows))

    def test_pr_enumeration_requires_completion(self):
        pr = self.pr()
        other = dict(pr, merge_commit_sha="b" * 40)
        request = Mock(side_effect=[[other] * 100, [pr], pr])
        self.assertEqual(versions.merged_pr_level(REVISION, request), (42, "patch"))
        self.assertIn("page=2", request.call_args_list[1].args[0])
        request = Mock(return_value=[other] * 100)
        with self.assertRaises(Refusal):
            versions.merged_pr_level(REVISION, request)
        self.assertEqual(request.call_count, 10)
        with self.assertRaises(Refusal):
            versions.merged_pr_level("main", Mock())

    def test_independent_annotated_tag_histories_and_bootstrap(self):
        tags = [self.tag("cma-web", "0.11.9"), self.tag("cma-web", "0.11.10"),
                self.tag("cma-agent-manager", "2.0.0"), {"name": "v99.0.0"}]
        web = versions.plan_version("web", "patch", tags, REVISION)
        am = versions.plan_version("am", "minor", tags, REVISION)
        eg = versions.plan_version("eg", "major", tags, REVISION)
        self.assertEqual((web.version, web.tag), ("0.11.11", "cma-web/v0.11.11"))
        self.assertEqual(am.version, "2.1.0")
        self.assertEqual(eg.version, "1.0.0")
        for component, package in versions.DEPLOYABLES.items():
            plan = versions.plan_version(component, "patch", [], REVISION)
            self.assertEqual(plan.version, "0.10.2" if component == "runtime" else "0.10.1")
            self.assertEqual(plan.repository, "ghcr.io/chronoaiproject/" + package)

    def test_mutable_malformed_duplicate_and_already_claimed_tags_fail(self):
        tag = self.tag("cma-web", "1.2.3")
        cases = [[dict(tag, type="commit")], [dict(tag, target_type="tag")],
                 [dict(tag, annotation_name="other/v1.2.3")], [dict(tag, target="main")],
                 [dict(tag, target=REVISION)], [self.tag("cma-web", "01.2.3")], [tag, tag], [None]]
        for tags in cases:
            with self.subTest(tags=tags), self.assertRaises(Refusal):
                versions.plan_version("web", "patch", tags, REVISION)
        for component, revision, tags in (("other", REVISION, []), ("web", "main", []), ("web", REVISION, {})):
            with self.assertRaises(Refusal):
                versions.plan_version(component, "patch", tags, revision)

    def test_empty_selector_skips_label_lookup_and_selected_list_is_exact(self):
        request = Mock()
        self.assertEqual(versions.plan_selected([], REVISION, request, []), (None, []))
        request.assert_not_called()
        pr = self.pr()
        number, plans = versions.plan_selected(["cma-agent-manager", "cma-event-gateway"], REVISION,
                                               Mock(side_effect=[[pr], pr]), [])
        self.assertEqual(number, 42)
        self.assertEqual([plan.component for plan in plans], ["am", "eg"])
        for selected in (["cma-web", "cma-web"], ["other"], [None], "web"):
            with self.subTest(selected=selected), self.assertRaises(Refusal):
                versions.plan_selected(selected, REVISION, request, [])
        request.assert_not_called()



if __name__ == "__main__":
    unittest.main()
