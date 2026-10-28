# Admitted Codex graceful EOF recovery

This provider helper implements the final native drain for an explicitly admitted
Sandbox recovery. The core owns admission, its immutable inspection plan, the
runtime generation change and reopening the original conversations on the same
home. A read-only inspection is not an idle proof or permission to interrupt work.

The adapter concatenates `codex/recovery_io.py` and `codex/recovery.py`, then sends
the source to `/usr/bin/python3 -I -c` through the bounded owned-Pod maintenance
transport. There is no installed recovery executable or new runtime service.
Request and response are each limited to 64 KiB; the helper has a 25-second total
deadline inside the transport's 30-second deadline. It returns fixed refusal
codes, process identities, conversation identities and hashes. It never emits
history content, command arguments, credentials, configuration or logs.

## Inspection and durable admission

Inspection takes version `1`, action `inspect`, a runtime object containing
`sandboxId`, `homeIdentity`, `generation`, `bootId`, and the exact ordered Agent
array containing `agentId` and `conversationId`. It does not take an operation ID.
It returns status `inspected`, the complete plan, and `planSha256`.

The array contains zero through sixteen live conversations. When the parent
runtime remains after all Agents are Deleted, core admission can supply an empty
array while retaining every terminal Agent record. This uses the same exact
boot, home, supervisor and full process-inventory proof as a live-conversation
request. An empty array permits only the supervisor, current inspector and
source-pinned relay; any leftover Agent, tool child or unrelated process causes
refusal. It does not declare the runtime Pod stopped. The admitted quiesce step
still writes and verifies its durable journal, rechecks the boot and process
absence through final proof, and returns an empty history array without sending
signals or reading, recreating or reopening a Deleted Agent's history. A replay
cannot bypass those checks. Core/store validation remains responsible for the
terminal child records and complete runtime-generation publication log proof.

The plan includes the boot identity, exact supervisor identity, Agent homes,
source and executable checksums, PID/start-time/parent identities, stdin ownership
evidence, the complete process baseline, and per-Agent history summaries. Each
history file has its device/inode identity, original size, SHA-256, native
conversation identity, last native turn marker and unresolved turn IDs. The
per-Agent `historySha256` is SHA-256 of canonical compact JSON for the ordered
file array projected to `path`, `conversationId`, `bytes`, and `sha256`.

The core persists the exact plan before quiescing. Quiesce takes the same runtime
and Agent array, action `quiesce`, `expectedPlan`, and its admitted
`sandbox-lifecycle-v1:sbx_<32hex>:<authority>` operation ID. The journal filename
is SHA-256 of that operation ID. The journal is bounded, written atomically and
fsynced beneath the persistent `.cma/recovery` directory before any signal.
Retries require the identical operation and plan, including after a lost response
or when the original bridge has already exited.

## Process and boot fencing

Only the source-pinned ACP Node process receives `SIGTERM`, through a Linux pidfd
which cannot retarget a reused PID. Neither the npm launcher nor the native
binary nor a process group receives a signal. There is no `SIGKILL` escalation.
Native app-server receives EOF on stdin and runs its own task shutdown and
persistence path. Killing the bridge avoids its ordinary stdin-close handler,
which would otherwise kill the child after two seconds.

Immediately before a signal, the helper holds the supervisor's existing
`workspace-removal-writer-v1.lock`, verifies its boot ownership JSON and home
binding, and verifies the exact original supervisor, Node, launcher, native
executable and command identities. Unsupported source digests, isolation-wrapper
topologies, shared stdin ownership and replaced processes are refused.

Linux Node uses Unix socketpairs for its child stdio. When UNIX_DIAG_PEER is
available, the helper checks the precise writer endpoint and every descriptor
owner. Some retained kernels return ENOENT because that diagnostic handler is
absent. For that specific result, the plan records `pinned_source_stdio` evidence:
the exact pinned ACP source creates one child with three stdio socketpairs, and
the exact pinned npm source passes those descriptors unchanged to native.
Inspection requires exactly three distinct, unbound, connected Unix stream
socket endpoints exclusively held by ACP; native's three distinct stdio endpoints
must be held exclusively by that native process and its npm launcher. All six
endpoints are disjoint and the complete descriptor inventory must remain stable.
Closing ACP closes all three parent endpoints. This proof never assumes adjacent
inode numbers. Missing, extra, named or shared endpoints are refused.

## Final history proof and limits

Inspection permits only PID 1's verified supervisor, the current inspector,
the exact requested Agent process trees and the source-pinned supervisor relay.
That existing control transport has exact `/usr/bin/python3 -I -u -c` arguments,
the checked `runtime_exec_relay.py` source, no Agent home and no children. Its
fixed implementation only relays stdio to the supervisor's loopback port. The
helper revalidates these service identities before signals and final proof.
An orphan or unrequested Agent, or an
unrelated user daemon, is a refusal. Such a process is never exempted merely
because its PID appeared in the inspection baseline. Success requires all
original native processes and descendants to disappear.
Surviving new or detached processes are refused, including processes that escaped
the original parent tree by reparenting. PID reuse and a changed supervisor boot
are refusals, never reasons to retarget recovery.

After exit, the helper streams complete JSONL histories, checks each native
identity, proves every original byte remains an exact prefix, and requires all
current native turns to have terminal completion or abort markers. It checks
process absence again after reading history. Existing history files, SQLite
databases, dirty Worktree files, configuration and the retained home are not
rewritten or deleted. Only the recovery journal is written.

Some retained histories contain abandoned start markers from an earlier native
process. Inspection records an explicit `historicalOpenTurns` baseline only when
the marker's timestamp is more than 60 seconds before the current native start,
derived from checked Linux boot time and process start ticks. In a currently open
history file, a later terminal turn marker must also follow the abandoned start
in both timestamp and file order. The pinned native implementation starts a cold
resume with no active old turn; the later terminal turn also establishes a
serial-turn boundary within an open conversation. Closed historical files need
the pre-incarnation timestamp proof. Missing timestamps and current-incarnation
turns receive no exemption. Final proof permits only those exact old start IDs
and positions, so reusing an old ID in a new start cannot bypass the terminal
requirement. It leaves abandoned markers untouched and invents no completion.

Process disappearance alone is insufficient: native can log shutdown timeout or
persistence errors and still exit normally. The helper therefore refuses missing,
partial, replaced, malformed or nonterminal history even after process exit.
Native does not persist a shutdown-complete marker. This evidence proves the
preserved durable transcript and terminal markers; it cannot reconstruct output
which native never persisted before a failure. An interrupted active turn is
explicitly admitted recovery behavior. A persisted active native goal can resume
on the replacement runtime according to its existing native state.

## Executable evidence

`recovery_test.py` covers full histories larger than 8 MiB, old-prefix corruption,
history and journal symlinks, malformed records, stale boots, changed executable
or parent identities, reused PIDs, exact stdin ownership, missing/extra/shared
socket endpoints, journal-before-signal ordering, lost response replay, already
exited bridges, bounded timeout without escalation, detached descendants and
refusal when final native turn markers are absent.
It also covers empty live-conversation inspection and journaled quiescence,
unchanged retained files, refusal of unknown or late processes, exact-plan replay
and stale-boot rejection without any signal.

`recovery_native.test.cjs` uses the pinned Codex 0.147.0 native binary and an
offline model fixture. It completes one turn, starts another with a real shell
child and dirty file, terminates only ACP, observes native's normal exit, verifies
the shell has stopped and history has a terminal abort marker, then reopens the
same conversation and completes another prompt. The original history is an exact
prefix and the dirty file hash remains unchanged. The observation-only Python
launcher in that test records the real native exit code without injecting RPCs.
The runtime image build runs both test suites.
