# CMA Project State

Audit date: **2026-11-02 (Asia/Singapore)**

GitHub Issues are the authoritative source for task state. This document records the evidence boundary and points to the Issues that contain the executable task definitions. It does not replace their acceptance checklists.

## Baseline and current phase

The audit uses `origin/develop` at `738e9d34c` (the merge of PR #1270) as its source baseline. The repository contract is `CLAUDE.md`; the service boundary and deployment documents remain the architectural authorities. The latest observed `develop` CI run for that revision passed all listed jobs, including backend, frontend, manifests, image builds, SDKs, secret scanning and the CMAEG latency budget: [CI run 41312067732](https://github.com/ChronoAIProject/cma/actions/runs/41312067732).

The current phase is operational hardening around the native Agent Manager and CMA Event Gateway split. The evidence priority is continuity of existing Agents, explicit provider and owner authorization, runtime capability guarantees, and reproducible acceptance at the public interfaces. Source code, a merged PR, or an old production note does not by itself prove that a current deployment provides the capability.

The audit preserved the user's existing dirty checkout. The task-entry documentation is being prepared from a clean `codex/ai-task-entry` worktree based on the baseline above.

## Verified or source-proven capabilities

- The four-layer ownership and dependency rules, fixed repository layout, public-interface boundaries, and deployment-surface requirements are current repository rules in [`CLAUDE.md`](../CLAUDE.md). They are the review constraints for every task below.
- The `develop` revision above has a passing CI run covering the repository's automated backend, frontend, manifest, image, SDK, secret and latency gates. This proves that revision's CI result; it does not prove live deployment state.
- The runtime contains a pinned upstream-built GitHub CLI and configures a local `api-github` proxy for managed runtimes. The checked-in contract states that API and GraphQL access through `gh` is supported while native Git transport is separate: [`CMA_GitHub_Runtime.md`](CMA_GitHub_Runtime.md) and [`CMA_GitHub_API_Workflow.md`](CMA_GitHub_API_Workflow.md). The contract test is local evidence; a live contribution flow remains a separate acceptance boundary.
- The authenticated runtime Agent capability exposes `agent_key_scope_revision` and, when document storage is configured, `publish_document` and `update_document`. Their authorization, replay, revision and size rules are covered by source tests and [`API_MCP.md`](API_MCP.md). This audit did not perform a live owner-authorized document publication.
- PR [#1235](https://github.com/ChronoAIProject/cma/pull/1235) merged the owner-key and provider-association implementation with focused tests and contract updates. Its own description says production activation and owned acceptance are still pending, so the implementation is recorded as merged while the live capability remains unverified.

## Known gaps and unverified claims

- [#843](https://github.com/ChronoAIProject/cma/issues/843) records measured runtime failures caused by missing `ps` and `jq`. It is the current ready task: the image package set, runtime probe, and documentation must be changed and verified together.
- [#1029](https://github.com/ChronoAIProject/cma/issues/1029) needs a NyxID contract for one-logical-reply payload limits, deadlines and receipts before CMA can implement and verify the relay change. It is blocked on that external contract.
- [#844](https://github.com/ChronoAIProject/cma/issues/844) and [#842](https://github.com/ChronoAIProject/cma/issues/842) contain older session reports about GitHub scope diagnosis and proactive guidance. The reports are useful evidence, but this audit did not reproduce them on the current deployment; the related permission and upstream conditions are not ready for autonomous implementation.
- PR [#1243](https://github.com/ChronoAIProject/cma/pull/1243) addresses the gateway partition/model failure reported by [#1242](https://github.com/ChronoAIProject/cma/issues/1242), but its description explicitly leaves the affected production mapping unverified.
- PR [#1239](https://github.com/ChronoAIProject/cma/pull/1239) addresses the idle-revival timeout reported by [#1164](https://github.com/ChronoAIProject/cma/issues/1164), but its description explicitly leaves production outbox delivery unverified.
- The current checkout contains matching uncommitted changes for [#1209](https://github.com/ChronoAIProject/cma/issues/1209) and frontend changes for [#1213](https://github.com/ChronoAIProject/cma/issues/1213). They are recorded as in-progress through the existing branch/workspace evidence and must not be duplicated by a new worker.

## Work in progress

- [#1276](https://github.com/ChronoAIProject/cma/issues/1276) is the single coordination Issue for the Agent eval closed loop already being developed in another session. The observed branch is `codex/agent-eval-loop`; no progress, cases, thresholds, live results, or PR are inferred until that session reports them.
- [#1209](https://github.com/ChronoAIProject/cma/issues/1209) has matching uncommitted skill-version error handling, tests and documentation in the shared checkout. It has no published PR at this audit.
- [#1213](https://github.com/ChronoAIProject/cma/issues/1213) has an existing `fix/web-new-agent-button` branch and a validation report in the Issue body, but no open PR at this audit.
- [#1242](https://github.com/ChronoAIProject/cma/issues/1242) is being addressed by PR [#1243](https://github.com/ChronoAIProject/cma/pull/1243); [#1164](https://github.com/ChronoAIProject/cma/issues/1164) is being addressed by PR [#1239](https://github.com/ChronoAIProject/cma/pull/1239). Neither Issue is closed by the existence of the PR.
- The remaining open Issue records are not silently promoted to work. In particular, [#1206](https://github.com/ChronoAIProject/cma/issues/1206) lacks a confirmed root cause for connection selection or binding, and [#1231](https://github.com/ChronoAIProject/cma/issues/1231), [#1225](https://github.com/ChronoAIProject/cma/issues/1225) and [#1228](https://github.com/ChronoAIProject/cma/issues/1228) retain deployment or external acceptance conditions after their related code changes.

## Candidate task pool

The Issue bodies contain the complete problem statement, evidence, scope, acceptance checks, dependencies and delivery boundary. The current small pool is:

1. [#843 — runtime image `ps`/`jq` guarantee](https://github.com/ChronoAIProject/cma/issues/843), **ready**. It has a measured user-impact report, a directly identified Dockerfile gap, a bounded code and documentation scope, local image verification, and no external credential dependency.
2. [#1209 — installed skill version upgrade error](https://github.com/ChronoAIProject/cma/issues/1209), **in-progress**. A matching change set is already in the shared checkout; a second implementation would conflict.
3. [#1213 — New Agent button during access refresh](https://github.com/ChronoAIProject/cma/issues/1213), **in-progress**. An existing named branch and validation report are present; it needs a PR boundary before another worker can claim it.
4. [#1029 — complete relay replies](https://github.com/ChronoAIProject/cma/issues/1029), **blocked**. NyxID payload and receipt semantics are not available in this audit.
5. [#1206 — selecting among multiple GitHub connections](https://github.com/ChronoAIProject/cma/issues/1206), **open and unready**. The report does not establish whether the cause is selection, caching, key delivery or UI state.

Only #843 is marked `status:ready` in this audit. The other entries are deliberately not marked ready because their work is already active, lacks a PR boundary, depends on an external contract, or lacks enough root-cause evidence. The project should add a second or third ready Issue only after those conditions change; the count must not be filled by speculation.

## Recommended next step

Claim [#843](https://github.com/ChronoAIProject/cma/issues/843) first. The acceptance target is an image and documentation change that makes `ps` and `jq` available in both runtime variants, proves their presence in CI image evidence, keeps existing runtime gates passing, and closes only after the PR is merged with that evidence. It does not require production credentials or a live sandbox.

The selection and verification procedure is [`AI_WORKFLOW.md`](AI_WORKFLOW.md).
