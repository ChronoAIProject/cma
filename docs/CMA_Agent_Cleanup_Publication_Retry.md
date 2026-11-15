# Agent cleanup publication conflicts

A cleanup step claims durable work for eighteen minutes. If its MongoDB
publication exhausts the bounded transaction retry budget with a definite abort,
the store returns `Conflict`. Previously the worker returned that error while
retaining the original claim. An otherwise healthy next worker had to wait for
lease expiry even when the external cleanup operation had already succeeded.

The worker now schedules the unchanged cleanup step after a short retry delay.
It rereads the exact Agent and requires the original unexpired holder, fence,
work, lifecycle state, stop authority, settings, hierarchy and conversation input.
Only concurrent display and log metadata may be adopted. Publication races have
eight bounded CAS attempts. The existing durable retry policy and attempt count
still control repeated failures.

Recovery itself performs no external cleanup. The next ordinary worker acquires
a fresh fence and uses the same stable operation IDs, hashes and existing
receipts. Unknown commits and store availability errors retain their existing
reconciliation and lease; they never authorize this release. Startup publication
is outside this recovery path. No deployment setting or persistence format changes.

The app-scoped real MongoDB failpoint regression covers all seven cleanup steps,
both a single transaction abort and exhaustion of the five-attempt budget. It
checks timely lease release, retained work and receipts, same-child deletion,
stable external request identities with fresh fences, and untouched active parent
Sandbox/ready Workspace. Boundary tests cover concurrent metadata, expired or
replacement authority, changed settings/input/stop intent, and unknown commits.

This reproduces a source-level cause of eighteen-minute cleanup stalls. Matching
production intervals alone do not establish the production cause; that requires
the corresponding worker error and durable lease evidence.
