# First-publication interruption qualification

The runtime's explicit local command
`--qualify-first-publication-interruption` uses the normal native assignment
receiver and `Supervisor` startup. It arms one process-local stop at the initial
ledger publication boundary after the staged database has closed and its tree
has synced, before creating the durable fresh intent. Normal
`--wait-for-assignment` never arms this stop. No environment variable, wire
request, or file content enables it. The official runtime image contains this
command; qualification uses the same release ELF as ordinary operation.

This command is exclusively for an isolated qualification runtime on a scratch
PVC. Production entrypoints and pool templates continue to use
`--wait-for-assignment`. The command writes and flushes this fixed stderr event:

```json
{"qualification":"first_publication_interruption","phase":"stage_synced_before_intent","action":"sigstop"}
```

It then raises process-wide `SIGSTOP`. A later `SIGCONT` returns an explicit
qualification refusal, so a continued qualification process cannot serve. A
startup that does not reach the hook also refuses before binding its listener.
The stop has no timeout of its own: the bounded external qualification owner
must kill and join the stopped process and perform its existing cleanup.

The release qualification must perform these steps for each shipped family:

1. Bind the actual merged source, image manifest, config and ELF. Start the
   explicit qualification command on a distinct scratch RWOP PVC with the
   production home initializer, subpath policy and family resources. Deliver an
   isolated test-owned bootstrap request through the real `--assign-runtime`
   receiver, using valid fixture identity and fresh local test credentials.
   This runtime regression does not claim an AM-issued assignment.
2. Join the event to the exact Pod UID, container ID and process identity.
   Observe that this process is stopped. The event alone does not prove a
   stop or a particular retained state. While it is stopped, verify the same
   PVC contains the staged ledger and no durable intent, committed manifest,
   or final ledger directory. Record the retained home identity and generation.
3. Kill and join that exact stopped process. Restart with the ordinary
   `--wait-for-assignment` command on the same PVC with the same test-owned
   home, identity and generation. Do not delete metadata or replace the
   assignment identity to make the test pass.
4. Observe successful recovery, public `/ready`, authenticated `RuntimeStatus`,
   installation, retained-home continuity and the existing receiver challenge,
   confirmation and refusal checks. An assignment transport ACK precedes ledger
   publication and does not prove runtime readiness or a durable serving commit.
5. Independently exercise durable-intent and committed-state preservation
   controls. Recovery may complete an eligible intent-bearing publication but
   must not discard it. A final database with its committed manifest removed
   remains ambiguous and must hold with its bytes preserved. These controls
   are separate from the positive first-run interruption case.

The pinned AM validates its immutable native Pod command as
`--wait-for-assignment`; it must not be bypassed to admit the qualification
command. A separate sequential integration case uses the unchanged actual AM,
its native Claim/home/PVC authority and the normal command with the same candidate
family image. That case records the public and durable AM result and receiver
checks. Its normal startup does not replace the interruption case above, and the
test-owned interruption assignment does not establish AM interruption recovery.
Both cases share one admitted infrastructure owner and run one runtime at a time.

The native tests exercise source checkpoints and the actual SIGSTOP/SIGCONT
mechanism in an isolated child. They do not substitute for the real-container,
same-PVC test above. Process interruption also does not establish storage
provider behavior under power loss or a failed filesystem sync.
