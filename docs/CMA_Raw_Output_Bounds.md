# Retained raw output storage bounds

`CHRONO_RUNTIME_RAW_OUTPUT_BOUNDS_ENABLED` is a strict boolean, default `false`
in code and the API ConfigMap. Both native and retained-provider transport
compositions override renderer preferences. Enabled attachments require retained
output negotiation before the ACP handshake.

The enabled policy shares one process-wide 16 MiB reservation pool for copied
raw retained-output payloads and queue nodes. Admission precedes the payload
copy, queue allocation, accepted source cursor and domain effects. The original
4 MiB per-frame payload limit and 8 MiB per-attachment startup wire limit remain.
The startup limit is checked before the copy as well, including when disabled.

Each reservation covers the requested payload bytes, the held-record layout,
two linked-node pointers and 128 bytes of conservative layout allowance. The
bounded startup queue uses independently allocated nodes. Popping frees the
node before returning its held record; queue teardown explicitly pops every
node before dropping its value. The held record drops its payload before
releasing its reservation. Empty payloads still reserve node storage. A drained
queue retains no uncharged backing capacity. Active delivery uses the same
reservation through bootstrap/capture and fingerprint creation, then releases
it when its owned raw copy is gone. The legacy queue representation is retained
when the flag is OFF.

An exhausted pool returns typed adapter capacity refusal before advancing the
accepted cursor or publishing a receipt/domain effect. It never waits to upgrade
a permit. The reader stops; no receipt, source ACK or legacy terminal is
manufactured. The runtime keeps its original unacknowledged source. Recovery
requires an explicitly supported fresh attachment with available capacity or a
compatible flag-OFF rollout. This policy adds no automatic reconnect loop and
does not erase the runtime journal or repeat an uncertain effect.

Release owners enable the flag separately after exact-head review, merge relay,
compatible deployment and coordinated qualification. Existing attachments keep
their original policy. The per-process pool is not a fleet capacity mapping;
capacity owners must qualify useful concurrent attachments and recovery.

This is the raw-copy/queue component for #1710, Part of #1472 and #1702. It is
requested storage accounting, not measured allocator usage or RSS. Incoming
WebSocket Messages, codec buffers, frame/fragment storage, decoder scratch,
prepared and decoded domain effects, permission state, durable assemblies and
public serialization are separate owners. A captured effect may remain alive
after this raw copy is released and must retain its own accounting. The final
ingress equation includes this 16 MiB component alongside permission retention
8 MiB and live fragments 32 MiB; the remaining components must share the rest of
the 128 MiB ceiling. This slice alone does not establish that aggregate ceiling
or the RSS acceptance criterion.

Contracts cover shared capacity across attachments, empty/maximal payloads,
startup boundary refusal, ordered drain, cancellation at a bootstrap barrier,
final storage release, same-cursor recovery after refusal, exact source
fingerprints and ACKs, required protocol negotiation, actual transport policy
override, combined raw/permission/JSON/shared-output/relay admission and refusal,
raw-copy lifetime across suspended bounded delivery, and legacy flag-OFF behavior.
No production campaign is part of this source change.
