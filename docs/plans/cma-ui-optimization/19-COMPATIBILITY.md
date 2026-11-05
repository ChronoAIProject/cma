# Ticket 19 compatibility and rollback floor

The supported recovery candidate must retain the complete task19 reader and
admission floor even when new UI/admission flags are disabled. A version1 schema
label or a successful decode alone is insufficient.

Required floor: durable task/lifecycle requests03/04, repository snapshots and
receipts, answer-bound deliverables, versioned feedback/reasons, answer-context
admission11/12, saved generation13, operation output14, browser-aware runtime
status15, desktop preparation journal16, control identity/revision17 and replay18.
Task19 policy adds no migration that may delete these records on disable.

The parent baseline decoder rejects an unnegotiated `agent_browser` runtime field.
The integrated runtime preserves the legacy status shape unless the caller sends
`Accept: application/vnd.chrono.cma.agent-computer-status+json`. This opt-in
advertises the new browser capability. Established filesystem and attachment
capabilities retain their original advertisement based on runtime support,
including for readers that do not request the computer extension. The separate
`agent-resources-status` media type opts in only to resource
preparation; callers can request both media types in one Accept header. Embedded
Workspace status remains in the legacy shape regardless of Accept. Both native
and provider transports request the new capabilities; retained runtimes can ignore
the header, leaving unsupported capabilities absent. This wire compatibility does
not lower the durable-record recovery floor.

The real old15 executable accepts its legacy positive control but fails on
retained desktop metadata. Neither is an approved rollback binary. No older
schema is substituted to make accepted work readable.

## November 4 production reader floor and writer admission

“Reversible while admissions are disabled” is not a valid API rollback condition.
The #1311 deployment at source `1f345ff2cf6016972ae00912ea19ee58327d9f07`
introduced ordinary writers outside the original task-admission checks. A disabled
policy did not prevent either strict predecessor-reader failure:

| Writer / trigger | Durable format and required reader | Admission after the safety fix |
| --- | --- | --- |
| Runtime tool status/title with output, or output-only updates | `agent_log` and update-checkpoint outcomes containing `ToolCallUpdated.output` or `ToolOutputReported`; retain both event readers, checkpoint recovery and wire negotiation | Agent owner's `replay_presentation`, checked in the durable recorder before folding new output |
| Ordinary feedback vote; reason edit | `agent_message_feedback` with `version`, `reasons`, `comment`, `updated_at`, `last_request`, or a null vote; retain expanded and legacy readers and request replay | Feedback owner's `replay_presentation` admits enrichment; unselected up/down votes use the legacy non-null shape |
| Turn completion from either log append path; suggestion worker | `agent_suggestions`, generated sets, usage/terminal receipts and accepted answer-context input | Agent owner's `suggested_followup_admission` gates enqueue and owner-filtered claims; generation rechecks that admission |

These are writer checks under the existing `CHRONO_TASK_EXPERIENCE_POLICY`, not
new configuration keys or read gates. Neither `task_home_admission` alone nor a
client's output preference admits these writers. No account is hardcoded.
At the migration-safety handoff, production was reported to select all five
capabilities only for the
owner (`76fe9d91-1f1d-4234-9352-819a7c28f709`) and yitian
(`e3da0a58-d7dd-4ec8-ad3a-faaf671653ad`); re-read the live policy before delivery.
Policy changes take effect through API rollout; drain every predecessor writer
before treating closure as complete. Default/empty selection is closed.

With feedback enrichment closed, unchanged votes and exact accepted reason
requests remain replayable. A clear of a legacy vote would require deleting its
row (the predecessor cannot read null); changing an already-expanded row would
write the closed format. Both changes return `forbidden` while closed. Existing
rows are retained unchanged; enabled accounts keep clear/edit behavior and CAS
revisions. No backfill, deletion, downgrade or schema relabeling occurs.

Output gating preserves status/title events, first-message/operation evidence,
source cursor checkpoints, acknowledgement ordering, and live text. All old/new
history and checkpoint readers remain. Unselected suggestion rows are not
claimed, expired or removed by the new worker; reads and exact already-claimed
settlement remain available, as does recovery of accepted answer-context input.
Closing admission does not erase an existing reader-floor obligation.

The concrete #1311-compatible API reader reference deployed November 4 is:

`us-central1-docker.pkg.dev/example-project-01/example-registry-02/cma-api@sha256:b0499635ea0345e513d2032108236be449787f144e1247d553647b2307c08756`

Its source is `1f345ff2cf6016972ae00912ea19ee58327d9f07`, tree
`e42b7ee42abc3e9b9a47c99b936ab2e566613f3a`. Retain this complete reader/recovery
floor in a forward recovery build that also contains writer admission. That
original image contains the ungated writers: its identity establishes the
reader reference, not writer-closure enforcement or a newly executed rollback
certification. Record the exact tested forward-fix digest before promotion.

The previous API image
`cma-api@sha256:2626145db5111aeede9c11d985dff5e468dc226542446ffcd4d283dfd5505601`
(source tree equivalent to `a85bc87c0d06a5ad59c78bb1741fa439a656563a`) is below
this floor. It rejects both new tool events/fields and expanded feedback.
Do not restore it based on disabled policy or an unchanged outer schema version.
Record absence has not been established in production and must not be assumed.
Later releases must raise the floor for every additional writer, including the
startup-failure shape in #1318; #1311 alone does not certify recovery from that
batched release. #1316 history reads must continue decoding retained new events.

An image-only web rollback may retain the compatible API/Gateway and every
old/new hashed-asset initializer, volume and mount. A generic Deployment undo
can remove those assets. Never downgrade runtime pins or discard client journals,
keys, homes or accepted operations to make an older binary start.

## Executed coexistence

37/38 run the separately retained pre-policy17 public reader executable
`9375180e6b29cb4578487ab6785b6497c88831f9a9d8fc382184dc6a5cf6f01e`
against the new run's actual Mongo records and same mounted native runtime/home.
Its source is `attempt-17/source-manifest.json` at source head4362c69f, with the
integrated17 production floor and the owned acceptance harness. It recovers the
original lifecycle key/receipt and reads authorized transcript, file content and
actual download, feedback/reasons, environment, browser context and control.
Each process exits with one named pass; current public/native assertions retain
all three turns and exact first/second/third dispatches.

38 loads actual17 built `index.html`/JS/CSS/font bytes on the same browser origin,
then restores the current bundle. `retained-assets-result.json` hashes every
served old asset. After authorized replay loading, its persisted cursor is
non-null; old and new assets preserve the exact cursor and Responses journal,
read third-turn history, expose the original file/vote, and Return Live without
changing transcript. It is an executed asset rollback/re-upgrade, not a screenshot
of a fixture.35/36 harness mistakes and their corrections are retained separately.

38 also switches actual enabled/default-disabled public routers sharing the same
stores and native process. Fresh creation is denied; original accepted receipt
recovery still needs fresh authority; input is disabled but explicit return and
lost-return status recovery succeed. Restoration does not duplicate a turn.
The final12 independent foreign/revoked reads are denied; reauthorization returns
the same three completed turns and original file remains readable.

This proves a separate compatible reader process, static-asset coexistence and
policy closure on retained work. It does not prove a cluster rollout, old worker
lease takeover, image replacement, live process restart or arbitrary old client.
Legacy AG-UI/Responses selection behavior and both typed SDK readers are covered
by their existing current tests/contract corpus; no claim says all old clients
can understand every newly opted-in record.

## Operator procedure (prepared, not executed deployment)

1. Freeze expansion; select default-disabled policy or remove the affected exact
   account cohort. Preserve durable operations/feedback/output and current homes.
2. Keep control status and explicit Return available; recover the original
   operation rather than replaying uncertain input. Wait for authoritative
   settlement/expiry; do not invent a success from an HTTP timeout.
3. Use a candidate retaining the reader floor above. Validate actual accepted
   keys, turn identities, file digest, vote/reasons, browser incarnation/context
   and selected transport before replacing UI/API. Do not restore baseline15.
4. Keep active Sandboxes on their pinned image; follow the existing admitted idle
   update lifecycle. Feature closure is not runtime termination or rollback of Mongo.
5. Re-enable only after reviewed repair and bounded cohort observation. Recover
   pending intents with their original identities and fresh authorization.

Human13 acceptance and parent final history/Standards/Spec review are prerequisites
to release eligibility, not conditions a rollback script may waive.
