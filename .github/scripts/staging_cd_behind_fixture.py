"""Synthetic develop pushes, their Staging CD runs and the shared tag store."""
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
from staging_cd_promotion import proof
from staging_cd_receipts import annotation
from staging_cd_receipts_tests import fixture_receipt, fixture_runtime_receipt
from staging_cd_reservations import pending_name
from staging_cd_set import private_document, release_set
from staging_cd_versions import DEPLOYABLES, REPOSITORY

HUMAN = {"login": "maintainer"}
BOT = {"login": "github-actions[bot]"}
WORKFLOW = ".github/workflows/staging-cd.yml"


class Develop:
    def __init__(self, count=8):
        self.server = AllocationServer(count, reserved=False)
        self.changes, self.runs, self.queues, self.finals, self.posts = [], {}, {}, {}, []
        self.landing = True
        # The release plan of each push; by default the whole adoption window.
        self.plan = lambda queue: queue
        self.read = self.server.request
        self.server.request = self.serve

    @property
    def head(self):
        return self.server.history().head

    def source(self, index):
        return self.server.sources[index]

    def view(self, run_id, attempt=None):
        run = self.runs[run_id]
        number = len(run["attempts"]) if attempt is None else attempt
        state = run["attempts"][number - 1]
        return {"id": run_id, "run_attempt": number, "event": "push", "head_branch": "develop",
                "head_sha": run["head"], "path": WORKFLOW, "status": state["status"],
                "triggering_actor": state["actor"], "repository": {"full_name": REPOSITORY}}

    def serve(self, path, payload=None, **kwargs):
        if path == "actions/workflows/staging-cd.yml/runs?branch=develop&per_page=100":
            return {"workflow_runs": [self.view(run_id) for run_id in sorted(self.runs, reverse=True)]}
        if not path.startswith("actions/runs/"):
            assert payload is None, path
            return self.read(path, **kwargs)
        parts = path.partition("?")[0].split("/")
        run_id = int(parts[2])
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
                return {"jobs": [{"name": "plan", "status": "completed", "conclusion": "success"},
                                 {"name": "allocate", "status": "completed" if state["allocate"] else "queued",
                                  "conclusion": state["allocate"]}]}
            return self.view(run_id, attempt)
        return self.view(run_id)

    def start(self, run_id, actor=HUMAN):
        self.runs[run_id]["attempts"].append({"status": "in_progress", "actor": actor, "allocate": None})

    def complete(self, *run_ids):
        for run_id in run_ids:
            self.runs[run_id]["attempts"][-1]["status"] = "completed"

    def push(self, components, *, transition=None, verified=()):
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

    def allocate(self, run_id, *, verified=()):
        """Plan as the run's latest attempt, which a re-run attempt repeats."""
        queue, attempt = self.queues[run_id], len(self.runs[run_id]["attempts"])
        assert self.head == queue["head"]
        snapshot = AllocationSnapshot(self.server, "fixture", Path.cwd(), AllocationIO(self.server.command))
        assert snapshot.load(event_source=queue["event_source"])
        proofs = {key: {"writer": self.head, "stamp": proof(private_document(
            release_set({key: self.finals[key]}))["components"])} for key in verified}
        matrix = allocate_queue(queue, SnapshotAPI(self.server, snapshot), snapshot, run_id, attempt, proofs=proofs)
        snapshot.commit()
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

