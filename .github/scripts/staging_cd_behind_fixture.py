"""Synthetic develop pushes, their Staging CD runs, staging writers and the shared tag store."""
from datetime import datetime, timedelta, timezone
import hashlib
import json
from pathlib import Path

from staging_cd_allocate import allocate_queue
from staging_cd_allocation_fixture import AllocationServer
from staging_cd_allocation_io import AllocationIO
from staging_cd_allocation_snapshot import AllocationSnapshot, SnapshotAPI
from staging_cd_common import Refusal
from staging_cd_github import GitHubNotFound
from staging_cd_history import DevelopHistory
from staging_cd_compatibility import GROUP
from staging_cd_promotion import PREFIX, inventory, proof, snapshot
from staging_cd_receipts import annotation
from staging_cd_receipts_tests import fixture_receipt, fixture_runtime_receipt
from staging_cd_reservations import pending_name
from staging_cd_set import private_document, release_set
from staging_cd_versions import DEPLOYABLES, REPOSITORY

HUMAN = {"login": "maintainer"}
BOT = {"login": "github-actions[bot]"}
WORKFLOW = ".github/workflows/staging-cd.yml"
REDEPLOY = ".github/workflows/redeploy-staging.yml"
ENVIRONMENT = "chrono-ai-staging"
EPOCH = datetime(2026, 11, 21, 13, 0, tzinfo=timezone.utc)


class Develop:
    def __init__(self, count=8):
        self.server = AllocationServer(count, reserved=False)
        self.changes, self.runs, self.queues, self.finals, self.posts = [], {}, {}, {}, []
        # Staging writers, newest first, and the jobs and statuses they link.
        self.deployments, self.statuses, self.jobs, self.dispatches = [], {}, {}, {}
        self.landing, self.minutes = True, 0
        # The release plan of each push; by default the whole adoption window.
        self.plan = lambda queue: queue
        self.read = self.server.request
        self.server.request = self.serve

    @property
    def head(self):
        return self.server.history().head

    def source(self, index):
        return self.server.sources[index]

    def now(self):
        self.minutes += 1
        return (EPOCH + timedelta(minutes=self.minutes)).strftime("%Y-%m-%dT%H:%M:%SZ")

    def later(self, minutes):
        """The clock reading `minutes` after the latest synthetic event."""
        return (EPOCH + timedelta(minutes=self.minutes + minutes)).timestamp()

    def view(self, run_id, attempt=None):
        run = self.runs[run_id]
        number = len(run["attempts"]) if attempt is None else attempt
        state = run["attempts"][number - 1]
        return {"id": run_id, "run_attempt": number, "event": "push", "head_branch": "develop",
                "head_sha": run["head"], "path": WORKFLOW, "status": state["status"],
                "triggering_actor": state["actor"], "run_started_at": state["started"],
                "updated_at": state.get("updated", state["started"]), "conclusion": state.get("conclusion"),
                "repository": {"full_name": REPOSITORY}, "head_repository": {"full_name": REPOSITORY}}

    def serve(self, path, payload=None, **kwargs):
        if path == "actions/workflows/staging-cd.yml/runs?branch=develop&per_page=100":
            return {"workflow_runs": [self.view(run_id) for run_id in sorted(self.runs, reverse=True)]}
        listing = f"deployments?environment={ENVIRONMENT}&per_page="
        if path.startswith(listing):
            count, _, page = path[len(listing):].partition("&page=")
            size, index = int(count), int(page or "1") - 1
            return [dict(row) for row in self.deployments[index * size:(index + 1) * size]]
        if path.startswith("deployments/"):
            return self.statuses[int(path.split("/")[1])]
        if path.startswith("actions/jobs/"):
            return dict(self.jobs[int(path.split("/")[2])])
        if not path.startswith("actions/runs/"):
            assert payload is None, path
            return self.read(path, **kwargs)
        parts = path.partition("?")[0].split("/")
        run_id = int(parts[2])
        if run_id in self.dispatches:
            return dict(self.dispatches[run_id])
        if run_id not in self.runs:
            raise GitHubNotFound("GitHub resource was not found")
        if parts[3:] == ["rerun"]:
            assert payload == {"enable_debug_logging": False}
            self.posts.append(run_id)
            if self.landing:
                self.start(run_id, BOT)
            raise Refusal("GitHub API response is invalid")  # GitHub answers 201 without a body.
        assert payload is None, path
        if parts[3:4] == ["attempts"]:
            attempt = int(parts[4])
            if attempt > len(self.runs[run_id]["attempts"]):
                raise GitHubNotFound("GitHub resource was not found")
            if parts[5:] == ["jobs"]:
                state = self.runs[run_id]["attempts"][attempt - 1]
                gate_jobs = [self.plan_job(state.get("plan", "success")), self.allocate_job(state["allocate"])]
                jobs = [dict(job, id=run_id * 100 + index, run_id=run_id, run_attempt=attempt,
                             head_sha=self.runs[run_id]["head"]) for index, job in enumerate(gate_jobs)]
                jobs.extend(dict(job) for job in state["jobs"])
                return {"jobs": jobs, "total_count": len(jobs)}
            return self.view(run_id, attempt)
        return self.view(run_id)

    def start(self, run_id, actor=HUMAN):
        self.runs[run_id]["attempts"].append({"status": "in_progress", "actor": actor, "allocate": None,
                                              "started": self.now(), "jobs": [], "plan": "success"})

    @staticmethod
    def plan_job(outcome):
        """The plan job: successful, stopped at its transient gate, failed in its plan step, or a given job."""
        if isinstance(outcome, dict):
            return dict(outcome, name="plan")
        steps = {"success": [("Reconcile first-parent merges with the shared selector", "success"),
                             ("Require a complete release plan", "skipped")],
                 "stopped": [("Reconcile first-parent merges with the shared selector", "success"),
                             ("Save trusted develop comparisons", "success"),
                             ("Require a complete release plan", "failure")],
                 "failed": [("Reconcile first-parent merges with the shared selector", "failure")]}[outcome]
        return {"name": "plan", "status": "completed", "conclusion": "success" if outcome == "success" else "failure",
                "steps": [{"name": name, "conclusion": conclusion} for name, conclusion in steps]}

    @staticmethod
    def allocate_job(outcome):
        """The allocate job: not started, successful, stopped at its settlement gate, failed, or a given job."""
        if isinstance(outcome, dict):
            return dict(outcome, name="allocate")
        if outcome is None:
            return {"name": "allocate", "status": "queued", "conclusion": None}
        steps = {"success": [("Reserve independent immutable component versions", "success"),
                             ("Require a settled allocation", "skipped"),
                             ("Route finalized components by contract", "success")],
                 "stopped": [("Reserve independent immutable component versions", "success"),
                             ("Require a settled allocation", "failure"),
                             ("Route finalized components by contract", "skipped")],
                 "failure": [("Reserve independent immutable component versions", "failure"),
                             ("Require a settled allocation", "skipped"),
                             ("Route finalized components by contract", "skipped")]}[outcome]
        return {"name": "allocate", "status": "completed", "conclusion": "success" if outcome == "success" else "failure",
                "steps": [{"name": name, "conclusion": conclusion} for name, conclusion in steps]}

    def complete(self, *run_ids):
        for run_id in run_ids:
            self.runs[run_id]["attempts"][-1].update(status="completed", updated=self.now())

    def push(self, components, *, transition=None, verified=None):
        """Merge one develop commit whose own inputs select `components`."""
        index = len(self.runs)
        source = self.source(index)
        self.server._history = DevelopHistory(source, list(reversed(self.server.sources[:index + 1])))
        if components:
            self.changes.append({"source": source, "tree": "e" * 40,
                                 "deployables": sorted(DEPLOYABLES[key] for key in components)})
        queue = {"head": source, "event_source": source, "changes": list(self.changes)}
        if transition is not None:
            queue["contract_transition"] = transition
        queue = self.plan(queue)
        run_id = 801 + index
        self.runs[run_id] = {"head": source, "attempts": []}
        self.queues[run_id] = queue
        self.start(run_id)
        return self.allocate(run_id, verified=verified)

    def unselect(self, index):
        """Apply a narrower selector to an already reconciled source."""
        self.changes = [change for change in self.changes if change["source"] != self.source(index)]
        for queue in self.queues.values():
            queue["changes"] = [change for change in queue["changes"] if change["source"] != self.source(index)]

    def allocate(self, run_id, *, verified=None):
        """Plan as the run's latest attempt, which a re-run attempt repeats.

        Without `verified` components, the plan reads the synthetic staging writers.
        """
        queue, attempt = self.queues[run_id], len(self.runs[run_id]["attempts"])
        assert self.head == queue["head"]
        store = AllocationSnapshot(self.server, "fixture", Path.cwd(), AllocationIO(self.server.command))
        assert store.load(event_source=queue["event_source"])
        if verified is None:
            proofs = inventory(self.server.request, ENVIRONMENT)
        else:
            proofs = {key: {"writer": self.head, "stamp": self.stamp({key})} for key in verified}
        matrix = allocate_queue(queue, SnapshotAPI(self.server, store), store, run_id, attempt, read_proofs=lambda: proofs or {})
        store.commit()
        self.runs[run_id]["attempts"][-1]["allocate"] = "success"
        tasks = {}
        for row in matrix["include"]:
            task, = json.loads(row["tasks"])
            tasks[row["component"]] = task
        return tasks

    def finalize(self, task):
        reservation = task["reservation"]
        component = reservation["component"]
        make = fixture_runtime_receipt if component.startswith("runtime") else fixture_receipt
        receipt = make(**{key: value for key, value in reservation.items() if key != "schema"})
        body = annotation(receipt)
        oid = hashlib.sha1(body.encode()).hexdigest()
        self.server.objects[oid] = {"sha": oid, "tag": receipt["tag"], "message": body,
                                    "object": {"type": "commit", "sha": receipt["source"]}}
        self.server.refs[receipt["tag"]] = oid
        del self.server.refs[pending_name(reservation)]
        self.finals[component] = receipt
        return receipt

    def refs(self):
        return {"refs/heads/develop": self.head, **{"refs/tags/" + name: oid for name, oid in self.server.refs.items()}}

    def controller(self):
        """The staging controller outcome, read as the catch-up check reads it."""
        return snapshot(self.server.request, ENVIRONMENT)

    def stamp(self, scope):
        return proof(private_document(release_set({key: self.finals[key] for key in scope}))["components"])

    def write(self, run_id, attempt, head, name, scope, outcome):
        """One staging deployment by a job; outcome is applied, failed, overtaken or running."""
        number = 7001 + len(self.deployments)
        job_id = 9001 + len(self.deployments)
        launch = {"applied": "success", "failed": "failure"}.get(outcome, "skipped")
        steps = [{"name": "Coordinate the immutable deployment set", "conclusion": launch}]
        if outcome == "applied":
            steps.append({"name": PREFIX + self.stamp(scope), "conclusion": "success"})
        job = {"id": job_id, "name": name, "run_id": run_id, "run_attempt": attempt, "head_sha": head,
               "status": "in_progress" if outcome == "running" else "completed",
               "conclusion": {"failed": "failure", "running": None}.get(outcome, "success"), "steps": steps}
        self.jobs[job_id] = job
        self.deployments.insert(0, {"id": number, "sha": head, "ref": "develop", "environment": ENVIRONMENT,
                                    "created_at": self.now()})
        self.statuses[number] = [{"state": "inactive"}, {
            "state": {"failed": "failure", "running": "in_progress"}.get(outcome, "success"),
            "log_url": f"https://github.com/{REPOSITORY}/actions/runs/{run_id}/job/{job_id}"}]
        return job

    def deploy(self, run_id, lane, outcome="applied"):
        """The deploy job of `lane` (a component or "contract") in the run's latest attempt."""
        run = self.runs[run_id]
        scope = GROUP if lane == "contract" else {lane}
        job = self.write(run_id, len(run["attempts"]), run["head"],
                         f"Promote {lane} / Deploy admitted components / Deploy set", scope, outcome)
        run["attempts"][-1]["jobs"].append(job)
        return job

    def redeploy(self, outcome="applied"):
        """A maintainer's manual redeployment of every current final at the develop head."""
        run_id = 901 + len(self.dispatches)
        self.dispatches[run_id] = {
            "id": run_id, "run_attempt": 1, "event": "workflow_dispatch", "head_branch": "develop",
            "head_sha": self.head, "path": REDEPLOY, "status": "completed",
            "repository": {"full_name": REPOSITORY}, "head_repository": {"full_name": REPOSITORY}}
        return self.write(run_id, 1, self.head, "deploy / Deploy set", set(self.finals), outcome)

