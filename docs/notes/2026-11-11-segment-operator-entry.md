# Explicit Agent log operator entry

Part of #1348 and #1350. These commands are code entry points, not deployment or production authorization. They never run at service startup or from a public GET. No command in this record has been run against production.

The binary recognizes explicit audit-unit, audit-cancel, audit-install and gate status/replace/close arguments. Unrecognized Agent-log arguments exit with a refusal rather than starting the API. Audit commands require the same bounded original Agent and audit identifiers on retries. Gate writes preserve S4's explicit expected-revision CAS and policy parser. No automatic new retry identity or revision exists.

Examples, using placeholders only:

```text
cma --print-agent-log-operator-role audit
cma --print-agent-log-operator-role gate-read
cma --print-agent-log-operator-role gate-write
cma --agent-log-audit-unit <agent-id> <original-audit-id>
cma --agent-log-audit-cancel <agent-id> <original-audit-id>
cma --agent-log-audit-install <agent-id> <original-audit-id>
cma --agent-log-gate-status agent_log_segments_v1
cma --agent-log-gate-close agent_log_segments_v1 <expected-revision>
cma --agent-log-gate-replace agent_log_segments_v1 <expected-revision> <policy-file>
```

The existing CHRONO_MONGODB_URI, CHRONO_MONGODB_DATABASE and explicit authority mode supply connection configuration. The role-print path does not connect or create roles. An operator must independently provision the reviewed authority; this source change applies no grants. Isolated mode requires exact operation-specific collections/actions. Explicit shared-schema mode uses the existing shared-schema verifier and still rejects foreign-database or cluster grants. Neither role grants DDL, deletion or source-log writes. Gate status has no gate mutation grant. Audit installation can replace certified projection/head metadata and its exact Agent envelope fence; it does not rewrite Agent-log events.

Before any operation, the entry verifies current authority, transaction topology and zero pending schema units. An absent, partial, incompatible or unavailable schema refuses before the operation. Gate actions additionally require gates and admissions to be present in S1's verified inventory. That inventory dependency is currently missing, so gate actions refuse until the corresponding S1 schema checkpoint is integrated and migrated. This guard must not be removed to activate a cohort.

One invocation has a 45-second outer deadline and bounded connection/server-selection waits. A timeout or unknown commit is reported as uncertain: inspect and resume the original audit ID or gate revision. Driver messages and source records are not printed; output contains operation status or S4's aggregate gate status only. Unit audit may persist audit/evidence progress; install explicitly publishes the completed generation under S1's exact CAS. These are writes, not read-only production probes.

S1 owns audit/rebuild semantics, complete output evidence in rebuilt generations, schema and the eventual migration checksum. The shared argument dispatcher now lives in S2's segment_operations module; no additional audit parser is required in S1. Shared migration-ledger registration reuses track B's step registry and remains separate from this operator entry. Production activation still requires complete caller wiring, local/CI qualification, review and release-owner execution.
