# ADM-072 admission control and bounded history

The case reads the current revision, operator stop, last change, write
availability and newest-first history through both AMs. Comparison covers the
durable admission fields only. The same public response includes live capacity
counters and separately revisioned ceilings; capacity can legitimately change
between reads without an admission-control change. Full responses, including
capacity, request IDs, both serving snapshots, attribution and timing, remain in
private observations. The shared request runner validates the actual AM reached.

Each response must contain exactly `min(revision, 20)` history entries. The
current control and head change must agree; revisions and old/new stop links
must be contiguous. Every change and stop is checked for unsigned safe integers,
nonempty actor, bounded nonblank UTF-8 reason without control characters, and
the source-defined stop revision/time/actor/reason relationships. Malformed or
null entries fail validation without turning into a property-access exception.

`admission_read` remains an authentic parent-required setup. It needs an exclusive
admission lease and at least 21 transitions created through the public API, so
the twenty-entry boundary is actually exercised. Those records must remain in
the same disposable database throughout the test. The parent supplies
`setWriteAvailability(boolean)` for its real admitted serving configuration;
there is no public API for changing this service-construction gate. A fake
availability response, a generic setup boolean, or a different empty database
cannot satisfy this contract. The case starts no application or native process.
The parent owns bounded mode selection, its evidence, and process finality.

The existing unavailable-writer variant now proves no durable effect. If the
initial gate is enabled, the case records mode intent before selecting disabled
writes. If it is already disabled, no mode transition is requested. Both AMs
must expose disabled writes and unchanged control before either POST. Each AM
then receives one original stop request with the baseline revision and a valid
reason. The response must be exactly HTTP 503 with
`admission_control_writes_unavailable`; both AMs must still expose the same
durable snapshot and disabled gate after each refusal. A changed revision or
history, wrong refusal, unexpected success, or unknown outcome stops further
dispatch. The runner retains original responses before checking status or
attribution, including responses rejected before the case can observe them.

A memoized finalizer restores the original gate once when a mode intent exists,
then independently reads both AMs. It can run outside an active case step. A lost
disable acknowledgement still requires that one restoration attempt. A lost
restoration acknowledgement stays failed even if the following readbacks look
correct, and never triggers a second attempt. Restoration, each AM read, and
parent cleanup failures are retained independently. A successful-looking refusal
that actually changed control is never repaired with a compensating command;
the case fails and the parent disposes its isolated data. Immutable seeded
history also remains until parent database disposal.

The setup counts remain six automatic public recipes / 31 parent-required
factories. Offline tests exercise the case with explicit response and mode
models; they establish no application or two-AM acceptance. The sole parent
must bind the real fixture and execute the original three steady repetitions
with complete serving evidence and final disposal before this row can pass.
