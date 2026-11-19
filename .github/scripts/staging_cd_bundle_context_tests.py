"""Generated names never replace current-byte identity and parsed syntax proof."""
import copy
import json
import os
import subprocess
import tarfile
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import staging_cd_content_policy_tests as fixtures
from staging_cd_content_policy_tests import digest, finding
from image_bundle_contexts import CHUNK, CONTEXT, PREFIX, PREFIXES, RULE, prove, scope
from image_content_identity import archive_identities, layer_identities
from image_content_policy import catalog_index, decisions
from image_secrets.common import ScanError
from tests.fixtures import docker_archive, tar_bytes

EXPRESSION = "left.secret===right.secret&&left.value===right.value&&compare(left.value,right.value);"
CANDIDATE = "==right.secret&&left.value===right.value&&compare"
PATH = PREFIX + CHUNK + "-AbCd_123.js"


def item(payload, path=PATH, candidate=CANDIDATE, **changes):
    fields = {"rule": RULE, "candidate_sha256": digest(candidate.encode()), **changes}
    return finding(path, payload, **fields)


def catalog(candidate=CANDIDATE):
    return {"schema": "cma-image-content-policy-v2", "scanner_version": "1.0.3", "gitleaks_version": "8.30.1",
            "allowlist_sha256": "a" * 64, "gitleaks_rules_sha256": "b" * 64, "entries": [],
            "bundle_candidates": [{"component": "web", "asset_prefix": prefix, "chunk": CHUNK,
                                   "rule": RULE, "candidate_sha256": digest(candidate.encode()),
                                   "syntax": CONTEXT, "classification": "public-material",
                                   "reason": "Synthetic parsed comparison fixture"} for prefix in PREFIXES]}


def index(policy):
    return catalog_index(policy, {**policy, "policy": policy})


class BundleSyntaxTests(unittest.TestCase):
    def test_real_parser_accepts_comparison_across_version_content_and_unicode_offsets(self):
        for prefix in ("", "const build='0.0.1';\n", "const label='\U0001f680';\n\n"):
            payload = (prefix + EXPRESSION).encode()
            row = item(payload, line=prefix.count("\n") + 1)
            self.assertEqual(prove(payload, [row]), {row["finding_id"]})

    def test_safe_duplicates_require_all_occurrences_and_each_finding_line(self):
        payload = (EXPRESSION + "\n" + EXPRESSION).encode()
        rows = [item(payload, line=n) for n in (1, 2, 3)]
        self.assertEqual(prove(payload, rows), {row["finding_id"] for row in rows[:2]})

    def test_literal_comment_template_regex_and_unsafe_duplicates_refuse(self):
        quoted = json.dumps(CANDIDATE)
        cases = ["const password=" + quoted + ";", "/*" + EXPRESSION + "*/",
                 "const text=`" + EXPRESSION + "`;", "const text=/" + CANDIDATE + "/;",
                 "const text=" + json.dumps(EXPRESSION) + ";"]
        for bad in cases:
            for text in (bad, EXPRESSION + "\n" + bad):
                with self.subTest(kind=cases.index(bad), duplicate=text.startswith(EXPRESSION)):
                    payload = text.encode()
                    self.assertEqual(prove(payload, [item(payload)]), set())

    def test_token_shaped_assignment_is_not_a_comparison(self):
        token = "".join(chr(65 + n % 26) + str(n % 10) for n in range(24))
        for name in ("api_key", "password", "client_secret", "access_token"):
            payload = ("const " + name + "=" + json.dumps(token) + ";").encode()
            self.assertEqual(prove(payload, [item(payload, candidate=token)]), set())

    def test_computed_optional_literal_wrong_operator_and_call_contexts_refuse(self):
        cases = [EXPRESSION.replace("left.secret", "left['secret']"),
                 EXPRESSION.replace("right.secret", "right?.secret"),
                 EXPRESSION.replace("left.value", "'literal'"),
                 EXPRESSION.replace("===", "=="), EXPRESSION.replace("&&", "||"),
                 EXPRESSION.replace("compare(left.value,right.value)", "compare(left.value)"),
                 EXPRESSION.replace("compare(left.value,right.value)", "compare(left.value,'literal')"),
                 EXPRESSION.replace("compare(", "compare?.("),
                 EXPRESSION.replace("compare(", "object.compare("),
                 EXPRESSION.replace("right.value);", "right['value']);")]
        for text in cases:
            with self.subTest(case=cases.index(text)):
                self.assertEqual(prove(text.encode(), [item(text.encode())]), set())

    def test_malformed_non_utf8_oversize_and_wrong_finding_refuse(self):
        payload = EXPRESSION.encode()
        row = item(payload)
        for bad in (b"", payload + b"\xff", payload + b" function {", b"{" + payload):
            self.assertEqual(prove(bad, [row]), set())
        for changes in ({"candidate_sha256": "f" * 64}, {"rule": "gitleaks.generic-api-key"}, {"line": 2}):
            self.assertEqual(prove(payload, [dict(row, **changes)]), set())
        with patch("image_bundle_contexts.MAX_BYTES", len(payload) - 1):
            self.assertEqual(prove(payload, [row]), set())

    def test_parser_failures_and_unexpected_output_refuse_without_diagnostics(self):
        payload = EXPRESSION.encode()
        row = item(payload)
        for error in (OSError(), subprocess.TimeoutExpired("node", 30)):
            with patch("image_bundle_contexts.subprocess.run", side_effect=error):
                self.assertEqual(prove(payload, [row]), set())
        for code, output in ((1, b"[]"), (0, b"invalid"), (0, b"{}"), (0, b"[1]"), (0, b'["unknown"]')):
            result = subprocess.CompletedProcess([], code, stdout=output, stderr=b"must not escape")
            with patch("image_bundle_contexts.subprocess.run", return_value=result):
                self.assertEqual(prove(payload, [row]), set())

    def test_parser_never_inherits_credentials_or_node_preloads(self):
        environment = {"PATH": os.environ["PATH"], "HOME": "/synthetic", "LANG": "C.UTF-8",
                       "GITHUB_TOKEN": "synthetic", "NODE_OPTIONS": "synthetic", "NODE_PATH": "synthetic"}
        result = subprocess.CompletedProcess([], 0, stdout=b"[]", stderr=b"")
        with patch.dict(os.environ, environment, clear=True), \
                patch("image_bundle_contexts.subprocess.run", return_value=result) as process:
            prove(EXPRESSION.encode(), [item(EXPRESSION.encode())])
        kwargs = process.call_args.kwargs
        self.assertEqual(set(kwargs["env"]), {"PATH", "HOME", "LANG"})
        self.assertEqual(kwargs["timeout"], 30)
        self.assertNotIn("shell", kwargs)


class BundleIdentityTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)

    def evaluate(self, payload, path=PATH, candidate=CANDIDATE, component="web", changes=None):
        archive = self.root / "image.tar"
        config = docker_archive(archive, [tar_bytes([(path, payload)])])
        fields = dict(changes or {})
        finding_path = fields.pop("path", path)
        row = item(payload, finding_path, candidate, **fields)
        report = {"findings": [row], "inventory": {"images": [{"config_digest": "sha256:" + config}]}}
        workspace = self.root / "identity"
        workspace.mkdir(exist_ok=True)
        contexts = {}
        identities = archive_identities(archive, workspace, report, contexts)
        return decisions(index(catalog()), component, [row], identities, contexts)

    def test_archive_current_bytes_and_new_names_get_fresh_decisions(self):
        for prefix in PREFIXES:
            for suffix in ("AbCd_123", "9876--xy"):
                payload = ("const version='" + suffix + "';" + EXPRESSION).encode()
                result = self.evaluate(payload, prefix + CHUNK + "-" + suffix + ".js")
                self.assertEqual(result[0]["classification"], "public-material")

    def test_unknown_rule_component_candidate_and_stale_file_identity_refuse(self):
        payload = EXPRESSION.encode()
        for changes in ({"rule": "gitleaks.generic-api-key"}, {"candidate_sha256": "f" * 64},
                        {"file_sha256": "f" * 64}, {"path": "path-sha256:" + "f" * 64}, {"scope": "config"}):
            self.assertEqual(self.evaluate(payload, changes=changes), [])
        for prefix in PREFIXES:
            for component in ("am", "eg", "runtime"):
                self.assertEqual(self.evaluate(payload, prefix + CHUNK + "-AbCd_123.js",
                                               component=component), [])
        token = "".join(str(n % 10) + chr(65 + n % 26) for n in range(24))
        payload = (EXPRESSION + "const password=" + json.dumps(token) + ";").encode()
        self.assertEqual(self.evaluate(payload, candidate=token), [])

    def test_outside_two_literal_roots_or_javascript_chunk_family_refuses(self):
        paths = [PATH.replace("/html/", "/other/"), PATH.replace(CHUNK, "Other"), PATH + ".map",
                 PATH.replace("AbCd_123", "short"), PATH.replace("assets/", "assets/nested/"),
                 "opt/" + PATH, PATH.replace(".js", ".JS"), PATH.replace(".js", ".txt"),
                 PATH.replace("/html/", "/current/").replace(".js", ".txt"),
                 PATH.replace("/html/", "/current/") + ".map"]
        for path in paths:
            self.assertIsNone(scope(path))
            self.assertEqual(self.evaluate(EXPRESSION.encode(), path), [])

    def test_links_and_nested_metadata_cannot_gain_syntax_proof(self):
        payload = EXPRESSION.encode()
        for kind in (tarfile.SYMTYPE, tarfile.LNKTYPE):
            layer = self.root / "layer.tar"
            layer.write_bytes(tar_bytes([("target", payload), (PATH, (kind, "/target"))]))
            row = item(payload, entry=1)
            contexts = {}
            identities = layer_identities(layer, {}, {1: [row]}, contexts)
            self.assertEqual(contexts, {})
            self.assertEqual(decisions(index(catalog()), "web", [row], identities, contexts), [])
        layer.write_bytes(tar_bytes([(PATH, payload)]))
        row = item(payload, path=PATH + "!/nested")
        contexts = {}
        self.assertEqual(layer_identities(layer, {}, {0: [row]}, contexts), {})

    def test_overwritten_entry_retains_its_own_syntax_and_identity(self):
        good = EXPRESSION.encode()
        bad = ("const password=" + json.dumps(CANDIDATE) + ";").encode()
        layer = self.root / "layer.tar"
        layer.write_bytes(tar_bytes([(PATH, bad), (PATH, good)]))
        rows = [item(bad, entry=0), item(good, entry=1)]
        contexts = {}
        identities = layer_identities(layer, {}, {n: [row] for n, row in enumerate(rows)}, contexts)
        self.assertEqual(set(contexts), {rows[1]["finding_id"]})
        result = decisions(index(catalog()), "web", rows, identities, contexts)
        self.assertEqual([r["finding_id"] for r in result], [rows[1]["finding_id"]])

    def test_real_catalog_decision_cannot_be_overridden_by_syntax(self):
        payload = EXPRESSION.encode()
        row = item(payload)
        policy = catalog()
        real = {"component": "web", "scope": "layer", "path": PATH, "file_sha256": digest(payload),
                "candidate_sha256": row["candidate_sha256"], "rule": RULE,
                "classification": "REAL", "reason": "Synthetic adversarial review"}
        policy["entries"] = [real]
        with self.assertRaises(ScanError):
            index(policy)
        policy["bundle_candidates"] = []
        result = decisions(index(policy), "web", [row], {row["finding_id"]: digest(payload)},
                           {row["finding_id"]: scope(PATH)})
        self.assertEqual(result[0]["classification"], "REAL")

    def test_malformed_duplicate_or_broadened_catalog_refuses(self):
        original = catalog()
        for field, value in (("component", "am"), ("asset_prefix", "usr/share/nginx/other/assets/"),
                             ("chunk", "*"), ("rule", "*"), ("syntax", "member-only"),
                             ("candidate_sha256", "*"), ("classification", "REAL"), ("reason", "")):
            policy = copy.deepcopy(original)
            policy["bundle_candidates"][0][field] = value
            with self.subTest(field=field), self.assertRaises(ScanError):
                index(policy)
        for rows in ({}, [original["bundle_candidates"][0]] * 2, [{}]):
            with self.assertRaises(ScanError):
                index(dict(original, bundle_candidates=rows))
        with self.assertRaises(ScanError):
            index(dict(original, schema="cma-image-content-policy-v1"))

    def test_completed_scan_gate_keeps_unknown_assignments_blocking(self):
        token = "".join(chr(65 + n % 26) + str(n % 10) for n in range(24))
        cases = [(EXPRESSION, CANDIDATE, False),
                 ("const password=" + json.dumps(token) + ";", token, True),
                 (EXPRESSION + "const password=" + json.dumps(CANDIDATE) + ";", CANDIDATE, True)]
        for text, candidate, blocked in cases:
            fixture = fixtures.ContentPolicyTests()
            fixture.setUp()
            self.addCleanup(fixture.doCleanups)
            payload = text.encode()
            row = item(payload, candidate=candidate)
            config = docker_archive(fixture.archive, [tar_bytes([(PATH, payload)])])
            fixture.report.update(archive_sha256=digest(fixture.archive.read_bytes()), findings=[row],
                                  inventory={"images": [{"config_digest": "sha256:" + config}]})
            fixtures.assess(fixture.report, "credential")
            policy = catalog()
            policy.update(fixture.report["policy"])
            fixture.catalog_path.write_text(json.dumps(policy))
            # This synthetic review pin exercises the gate only; it cannot enable
            # repository policy or authorize a real archive.
            with patch.object(fixtures.gate, "load_content_policy",
                              return_value=(fixture.catalog_path, "a" * 64)):
                if blocked:
                    with self.assertRaises(fixtures.gate.ScanRefusal):
                        fixtures.gate.scan_archive(fixture.archive, fixture.output, fixture.scanner,
                                                   blocking_rules="credential", component="web")
                else:
                    fixtures.gate.scan_archive(fixture.archive, fixture.output, fixture.scanner,
                                               blocking_rules="credential", component="web")
            result = json.loads((fixture.output / "REPORT.json").read_bytes())
            self.assertEqual(result["counts"]["blocking"], int(blocked))
            self.assertEqual(result["findings"], [row])
            self.assertEqual(result["allowlisted"], [])


if __name__ == "__main__":
    unittest.main()
