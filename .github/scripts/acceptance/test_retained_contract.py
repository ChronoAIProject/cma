"""Permanent document slots never admit replacement or second source writes."""

import contextlib
import io
import json
import unittest

import documents
import retained
from safety import Refusal
from test_support import context, restart


def seed(p, actor, edit=True):
    doc = {"id": "test-doc-" + actor, "owner_subject": "test-" + actor, "revision": 1}
    body = {"title": "CMA acceptance fixture", "markdown": "Synthetic."}
    label = "document_" + actor
    def receipt(suffix, method, path, value):
        intent = {"tenant": actor, "path": path, "method": method, "body": value}
        retained.reserve(p, actor, label + suffix, intent)
        retained.observed(p, actor, label + suffix, intent, 201 if method == "POST" else 200, doc)
        p.state["writes"][label + suffix] = dict(intent=intent, key=label + suffix,
            status="complete", response_status=201 if method == "POST" else 200, response=dict(doc))
        p.fixture(label, dict(doc))
    receipt("_create", "POST", "/api/v1/documents", body)
    if edit:
        doc["revision"] = 2
        receipt("_edit", "PATCH", "/api/v1/documents/" + doc["id"], dict(body, expected_revision=1))


class PermanentFixtureTests(unittest.TestCase):
    def test_absent_local_or_live_fixture_refuses_before_either_actor_mutates(self):
        for phase in (documents.run, documents.updates):
            for absent in ("local", "live"):
                with self.subTest(phase=phase.__name__, absent=absent), context() as p:
                    seed(p, "a"); seed(p, "b")
                    if absent == "local":
                        del p.state["fixtures"]["document_b"]
                    before = retained.load(p); methods = []
                    def request(origin, path, method="GET", **kwargs):
                        methods.append(method)
                        if path.endswith("test-doc-b"):
                            return 404, {}, b'{"code":"document_not_found"}', 0
                        return 200, {}, json.dumps(p.state["fixtures"]["document_a"]).encode(), 0
                    p.http.request = request
                    with self.assertRaises(Refusal):
                        phase(p)
                    self.assertTrue(all(method == "GET" for method in methods))
                    self.assertEqual(retained.load(p), before)

    def test_consumed_slots_reuse_receipts_without_dispatch_after_restart(self):
        with context() as p:
            seed(p, "a"); restart(p)
            for suffix in ("_create", "_edit"):
                entry = p.state["writes"]["document_a" + suffix]; intent = entry["intent"]
                response = p.write("document_a" + suffix, "a", intent["path"], intent["method"], intent["body"])
                self.assertEqual(response[1], {"Acceptance-Retained": "true"})
                with self.assertRaisesRegex(Refusal, "consumed_document_slot"):
                    p.request("a", intent["path"], intent["method"], intent["body"], key=entry["key"])
                with self.assertRaisesRegex(Refusal, "retained_fixture_cap"):
                    p.write("replacement" + suffix, "a", intent["path"], intent["method"], intent["body"])

    def test_consumed_edit_updates_are_read_only(self):
        with context() as p, contextlib.redirect_stdout(io.StringIO()):
            seed(p, "a"); seed(p, "b"); methods = []
            def request(origin, path, method="GET", **kwargs):
                methods.append(method)
                actor = "a" if "test-doc-a" in path else "b"
                value = {"markdown": "Synthetic."} if path.endswith("/source") else p.state["fixtures"]["document_" + actor]
                return 200, {}, json.dumps(value).encode(), 0
            p.http.request = request
            documents.updates(p)
            self.assertEqual(methods, ["GET"] * 4)
            self.assertEqual(sum(row["result"] == "BLOCKED" for row in p.results), 2)

    def test_missing_consumed_fixture_refuses_original_cached_key(self):
        with context() as p:
            seed(p, "a"); del p.state["fixtures"]["document_a"]
            before = retained.load(p)
            for suffix in ("_create", "_edit"):
                intent = p.state["writes"]["document_a" + suffix]["intent"]
                with self.assertRaisesRegex(Refusal, "consumed_document_slot"):
                    p.write("document_a" + suffix, "a", intent["path"], intent["method"], intent["body"])
            self.assertEqual(retained.load(p), before)

    def test_unused_slots_still_allow_one_create_and_one_edit(self):
        with context() as p:
            methods = []
            def request(origin, path, method="GET", **kwargs):
                methods.append(method)
                value = {"id": "test-doc-a", "owner_subject": "test-a", "revision": len(methods)}
                return 201 if method == "POST" else 200, {}, json.dumps(value).encode(), 0
            p.http.request = request
            body = {"title": "CMA acceptance fixture", "markdown": "Synthetic."}
            response = p.write("document_a_create", "a", "/api/v1/documents", "POST", body)
            p.fixture("document_a", json.loads(response[2]))
            p.write("document_a_edit", "a", "/api/v1/documents/test-doc-a", "PATCH", dict(body, expected_revision=1))
            self.assertEqual(methods, ["POST", "PATCH"])


if __name__ == "__main__":
    unittest.main()
