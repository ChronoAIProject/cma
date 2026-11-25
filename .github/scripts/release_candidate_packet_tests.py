"""Synthetic release inventory and independent evidence checks for #1482."""
import copy
from datetime import datetime, timedelta, timezone
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

from release_candidate_packet import EvidenceFiles, artifact, component_identity, digest, main, verify_packet
from staging_cd_common import Refusal
from staging_cd_receipts import canonical, sha256
from staging_cd_receipts_tests import fixture_receipt, fixture_runtime_receipt
from staging_cd_targets import TARGETS


def write(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    data = canonical(value).encode()
    path.write_bytes(data)
    return digest(data)


def time_at(minutes):
    return (datetime(2026, 11, 20, tzinfo=timezone.utc) + timedelta(minutes=minutes)).isoformat(
        timespec="milliseconds").replace("+00:00", "Z")


def git(root, *args):
    return subprocess.check_output(["git", "-C", str(root), *args], text=True).strip()


class Packet:
    def __init__(self, root):
        self.root = root
        self.repository = root / "repository"
        self.repository.mkdir()
        git(self.repository, "init", "-q")
        git(self.repository, "-c", "user.name=Test", "-c", "user.email=test@example.invalid",
            "commit", "--allow-empty", "-qm", "old")
        old = git(self.repository, "rev-parse", "HEAD")
        git(self.repository, "-c", "user.name=Test", "-c", "user.email=test@example.invalid",
            "commit", "--allow-empty", "-qm", "candidate")
        source = git(self.repository, "rev-parse", "HEAD")
        tree = git(self.repository, "rev-parse", "HEAD^{tree}")
        self.evidence = root / "evidence"
        self.evidence.mkdir()
        receipts = {}
        for component in sorted(TARGETS):
            factory = fixture_runtime_receipt if component.startswith("runtime") else fixture_receipt
            receipts[component] = factory(component=component, source=source, tree=tree, build=sha256(component))
        repair = fixture_receipt(component="am", source=old, tree=tree, build=sha256("repair"))
        repair["digest"] = "sha256:" + sha256("repair-image")
        self.publications = {"schema": "cma-release-publications-v1", "components": receipts, "repair": repair}
        self.freeze = {
            "schema": "cma-release-candidate-v1", "source": source, "environment": sha256("environment"),
            "frozen_at": time_at(100), "components": {k: component_identity(v) for k, v in receipts.items()},
            "repair": component_identity(repair), "state": {},
            "previous_candidate_sha256": None, "checks": {},
        }
        observed_components = {key: {"identity": component_identity(receipt),
                                    "transport": {"source_digest": receipt["digest"], "destination_digest": receipt["digest"], "verified": True},
                                    "deployed_digest": receipt["digest"],
                                    "controller_verified": True} for key, receipt in receipts.items()}
        self.observation = {"schema": "cma-release-candidate-observation-v1", "environment": self.freeze["environment"],
                            "observed_at": time_at(150), "components": self.ref("components.json", observed_components),
                            "state": {}, "checks": {}, "recoveries": []}
        state = {
            "configuration": {"replicas": 2}, "flags": {"example": False}, "schema": {"version": 3},
            "external_versions": {"dependency": "1.0"}, "reader_floors": {"reader": "current"},
            "runtime_contract": {"runtime_contract": "3", "bootstrap": "1", "home_layout": "2"},
        }
        for key, value in state.items():
            ref = self.ref("state/" + key + ".json", value)
            self.freeze["state"][key] = ref["sha256"]
            self.observation["state"][key] = ref
        web = self.freeze["components"]["web"]
        previous = {"source": old, "digest": "sha256:" + sha256("old-web"),
                    "assets": {"assets/old.js": sha256("old-js")}}
        current = {"source": web["source"], "digest": web["digest"],
                   "assets": {"assets/new.js": sha256("new-js")}}
        serving = dict(current, assets=previous["assets"] | current["assets"])
        self.observation["web_assets"] = {
            key: self.ref("web/" + key + ".json", value)
            for key, value in (("previous", previous), ("current", current), ("serving", serving))
        }
        self.freeze["web_assets"] = {"previous_source": old, "previous_digest": previous["digest"],
            "previous_manifest_sha256": self.observation["web_assets"]["previous"]["sha256"],
            "candidate_manifest_sha256": self.observation["web_assets"]["current"]["sha256"]}
        self.matrix(artifact(receipts["am"]), artifact(repair), tree)
        checks = [("required-ci", "ci", 1, {"run_id": 101, "head_sha": source, "conclusion": "success"}),
                  ("retained-fixture", "fixture", 2, {"passed": True, "failures": 0})]
        checks += [(engine, "browser", 20, {"engine": engine, "asset_404": 0, "blank_pages": 0,
                                             "duplicate_commands": 0, "lost_drafts": 0})
                   for engine in ("chromium", "firefox", "webkit")]
        for name, kind, samples, outcome in checks:
            self.freeze["checks"][name] = {"kind": kind, "minimum_samples": samples, "inheritance": None}
            self.observation["checks"][name] = {
                "result": self.ref("checks/" + name + ".json", dict(self.common(samples), name=name, kind=kind,
                                                                      outcome=outcome)),
                "impact": None,
            }
        self.observation["candidate_sha256"] = self.candidate_sha()
        state_hashes = {name: sha256(name) for name in ("database", "history", "homes")}
        for index, start in enumerate((10, 40)):
            row = dict(self.common(1), run_id=sha256(str(index)), kind="initial_qualification",
                       repair=artifact(repair), started_at=time_at(start), first_failure_at=time_at(start + 1),
                       finished_at=time_at(start + 8), retained_before=state_hashes, retained_after=state_hashes,
                       database_rollback=False, history_rewritten=False, home_deleted=False,
                       asset_404=0, new_turn_available=True,
                       probes=[{"at": time_at(start + 2 + offset / 2), "ready": True} for offset in range(11)])
            self.observation["recoveries"].append(self.ref("recovery/" + str(index) + ".json", row))

    def candidate_sha(self):
        return sha256({key: self.freeze[key] for key in ("source", "environment", "components", "state",
                                                        "repair", "web_assets", "matrix")})

    def common(self, samples):
        return {"candidate_sha256": self.candidate_sha(), "environment": self.freeze["environment"],
                "started_at": time_at(1), "finished_at": time_at(9), "samples": samples,
                "status": "PASS", "attempt": 1, "cancelled": False}

    def ref(self, relative, value):
        return {"path": relative, "sha256": write(self.evidence / relative, value)}

    def replace(self, reference, mutate):
        previous = json.loads((self.evidence / reference["path"]).read_bytes())
        mutate(previous)
        reference.update(self.ref(reference["path"], previous))

    def matrix(self, candidate, repair, tree):
        sources = {"schema": "cma-reader-writer-source-inventory-v1",
                   "old": {"source": repair["source"], "tree": tree},
                   "new": {"source": candidate["source"], "tree": tree},
                   "collections": [{"name": "sample", "old": {"role": "runtime"}, "new": {"role": "runtime"}}]}
        source_ref = self.ref("matrix/source.json", sources)
        inventory = {"schema": "cma-migration-shape-inventory-v1", "source_inventory_sha256": source_ref["sha256"],
                     "sources": {"old": sources["old"], "new": sources["new"]},
                     "collections": [{"name": "sample", "review": "complete", "shapes": [
                         {"id": "ordinary", "definition_sha256": sha256("shape"),
                          "writers": [{"role": "old", "policies": ["retained"]},
                                      {"role": "new", "policies": ["disabled", "enabled"]}]}]}]}
        inventory_ref = self.ref("matrix/inventory.json", inventory)
        readers = [{"id": role, "role": role, "artifact": item} for role, item in
                   (("historical", repair), ("current", repair), ("candidate", candidate), ("recovery", repair))]
        fixtures = [{"id": policy, "sha256": sha256(policy), "policy": policy,
                     "writer": repair if policy == "retained" else candidate}
                    for policy in ("retained", "disabled", "enabled")]
        cells = [{"id": reader["id"] + "-" + fixture["id"], "reader": reader["id"],
                  "fixture": fixture["id"], "expectation": "accept"} for reader in readers for fixture in fixtures]
        plan = {"schema": "cma-migration-matrix-plan-v1", "candidate": candidate,
                "readers": readers, "fixtures": fixtures, "cells": cells}
        plan_ref = self.ref("matrix/plan.json", plan)
        bindings = {"schema": "cma-migration-shape-bindings-v1", "inventory_sha256": inventory_ref["sha256"],
                    "plan_sha256": plan_ref["sha256"], "bindings": [
                        {"collection": "sample", "shape": "ordinary", "writer": "old" if fixture["policy"] == "retained" else "new",
                         "policy": fixture["policy"], "fixture": fixture["id"], "fixture_sha256": fixture["sha256"],
                         "definition_sha256": sha256("shape")} for fixture in fixtures]}
        bindings_ref = self.ref("matrix/bindings.json", bindings)
        observations = []
        for cell in cells:
            reader = next(row for row in readers if row["id"] == cell["reader"])
            fixture = next(row for row in fixtures if row["id"] == cell["fixture"])
            row = {"id": cell["id"], "reader": reader["artifact"], "writer": fixture["writer"],
                   "fixture_sha256": fixture["sha256"], "policy": fixture["policy"], "execution": "artifact",
                   "outcome": "accept", "attempt": 1, "samples": 1, "started_at": time_at(1),
                   "finished_at": time_at(2), "checks": {name: True for name in
                       ("decode", "replay", "restart", "takeover", "identity_preserved", "writer_policy")}}
            raw = self.ref("matrix/" + cell["id"] + ".json", row)
            raw["path"] = cell["id"] + ".json"
            observations.append(dict(row, raw=raw))
        evidence_ref = self.ref("matrix/evidence.json", {"schema": "cma-migration-matrix-evidence-v1",
            "plan_sha256": plan_ref["sha256"], "candidate": candidate, "environment": "isolated-fixture",
            "configuration_sha256": self.freeze["state"]["configuration"], "started_at": time_at(1),
            "finished_at": time_at(2), "cells": observations})
        self.observation["matrix"] = {"source_inventory": source_ref, "shape_inventory": inventory_ref,
                                      "plan": plan_ref, "bindings": bindings_ref, "evidence": evidence_ref}
        self.freeze["matrix"] = {key + "_sha256": reference["sha256"]
                                 for key, reference in self.observation["matrix"].items() if key != "evidence"}

    def verify(self):
        data = canonical(self.freeze).encode()
        return verify_packet(data, digest(data), self.publications, self.observation,
                             EvidenceFiles(self.evidence), self.repository)

    def cli(self, expected_hash=None):
        freeze = self.root / "freeze.json"
        pinned = write(freeze, self.freeze)
        publications = self.root / "publications.json"
        write(publications, self.publications)
        observation = self.evidence / "index.json"
        write(observation, self.observation)
        output = self.root / "report.json"
        status = main(["--freeze", str(freeze), "--freeze-sha256", expected_hash or pinned,
                       "--publications", str(publications), "--observation", str(observation),
                       "--output", str(output), "--repository", str(self.repository)])
        return status, output


class ReleaseCandidateTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.packet = Packet(Path(self.temp.name))

    def mutation(self, name, ref, change):
        with self.subTest(name=name):
            self.packet.replace(ref, change)
            with self.assertRaises((Refusal, ValueError)):
                self.packet.verify()

    def test_complete_synthetic_packet_runs_real_matrix_and_writes_private_report(self):
        status, output = self.packet.cli()
        self.assertEqual(status, 0)
        report = json.loads(output.read_text())
        self.assertEqual(report["status"], "CONSISTENT")
        self.assertEqual(report["scope"], "reviewed_inventory_and_evidence_consistency")
        self.assertEqual(report["components"], sorted(TARGETS))
        self.assertEqual(report["reader_matrix"]["cells"], 12)
        self.assertEqual(report["recovery_rehearsals"]["initial_qualifications"], 2)
        self.assertEqual(report["retained_assets"], 1)
        self.assertEqual(output.stat().st_mode & 0o777, 0o600)

    def test_refuses_component_omission(self):
        del self.packet.publications["components"]["eg"]
        with self.assertRaises(Refusal):
            self.packet.verify()

    def test_refuses_changed_deployed_digest(self):
        self.mutation("digest", self.packet.observation["components"],
                      lambda row: row["am"].update(deployed_digest="sha256:" + "a" * 64))

    def test_refuses_incompatible_repair(self):
        self.mutation("repair", self.packet.observation["recoveries"][0],
                      lambda row: row["repair"].update(image="registry.example/api@sha256:" + "a" * 64))

    def test_refuses_missing_retained_asset(self):
        self.mutation("retained", self.packet.observation["web_assets"]["serving"],
                      lambda row: row["assets"].pop("assets/old.js"))

    def test_refuses_duplicated_rehearsal(self):
        refs = self.packet.observation["recoveries"]
        refs[1] = copy.deepcopy(refs[0])
        with self.assertRaises(Refusal):
            self.packet.verify()

    def test_refuses_cancelled_required_ci(self):
        self.mutation("cancelled", self.packet.observation["checks"]["required-ci"]["result"],
                      lambda row: row.update(cancelled=True))

    def test_refuses_missing_required_check(self):
        del self.packet.observation["checks"]["retained-fixture"]
        with self.assertRaises(Refusal):
            self.packet.verify()

    def test_refuses_recovery_window_shortfall(self):
        self.mutation("probe gap", self.packet.observation["recoveries"][0],
                      lambda row: row["probes"].pop(3))

    def test_refuses_frozen_recheck_as_initial_qualification(self):
        def change(row):
            row["kind"] = "frozen_recheck"
            row["started_at"], row["first_failure_at"], row["finished_at"] = time_at(101), time_at(102), time_at(109)
            row["probes"] = [{"at": time_at(103 + offset / 2), "ready": True} for offset in range(11)]
        self.mutation("recheck", self.packet.observation["recoveries"][1], change)

    def test_refuses_configuration_drift_and_wrong_frozen_hash(self):
        self.mutation("flags", self.packet.observation["state"]["flags"], lambda row: row.update(example=True))
        status, output = self.packet.cli("0" * 64)
        self.assertEqual(status, 1)
        self.assertFalse(output.exists())

    def test_inheritance_requires_exact_reviewed_impact_and_prior_identity(self):
        packet = self.packet
        prior = sha256("previous")
        prior_source = packet.publications["repair"]["source"]
        packet.freeze["previous_candidate_sha256"] = prior
        rule = packet.freeze["checks"]["required-ci"]
        impact = packet.ref("checks/impact.json", {"name": "required-ci", "previous_candidate_sha256": prior,
            "current_source": packet.freeze["source"], "current_candidate_sha256": packet.candidate_sha(),
            "decision": "inherit", "scope": "unaffected"})
        rule["inheritance"] = {"candidate_sha256": prior, "source": prior_source, "impact_sha256": impact["sha256"]}
        check = packet.observation["checks"]["required-ci"]
        check["impact"] = impact
        packet.replace(check["result"], lambda row: (row.update(candidate_sha256=prior),
                                                    row["outcome"].update(head_sha=prior_source)))
        self.assertEqual(packet.verify()["status"], "CONSISTENT")
        packet.replace(check["impact"], lambda row: row.update(scope="affected"))
        with self.assertRaises(Refusal):
            packet.verify()


if __name__ == "__main__":
    unittest.main()
