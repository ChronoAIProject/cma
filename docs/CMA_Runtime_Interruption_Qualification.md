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
   explicit qualification command in the existing isolated real-AM fixture
   with native Claim/home authority and the production PVC/subpath policy.
   Deliver the assignment through the real receiver.
2. Join the event to the exact Pod UID, container ID and process identity.
   Observe that this process is stopped. The event alone does not prove a
   stop or a particular retained state. While it is stopped, verify the same
   PVC contains the staged ledger and no durable intent, committed manifest,
   or final ledger directory. Record the retained home identity and generation.
3. Kill and join that exact stopped process. Restart with the ordinary
   `--wait-for-assignment` command on the same PVC and supported retained
   Claim/home identity. Do not delete metadata or invent replacement assignment
   authority to make the test pass.
4. Observe successful recovery, public `/ready`, authenticated `RuntimeStatus`, the public
   and durable AM result, retained-home continuity, and the existing receiver
   challenge, confirmation and refusal checks. An assignment transport ACK
   precedes ledger publication and does not prove runtime readiness or a
   durable serving commit.
5. Independently exercise durable-intent and committed-state preservation
   controls. Recovery may complete an eligible intent-bearing publication but
   must not discard it. A final database with its committed manifest removed
   remains ambiguous and must hold with its bytes preserved. These controls
   are separate from the positive first-run interruption case.

The native tests exercise source checkpoints and the actual SIGSTOP/SIGCONT
mechanism in an isolated child. They do not substitute for the real-container,
same-PVC test above. Process interruption also does not establish storage
provider behavior under power loss or a failed filesystem sync.
