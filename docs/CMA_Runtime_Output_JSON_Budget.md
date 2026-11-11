# Hosted output JSON structure budget

Part of #1472. `CHRONO_RUNTIME_OUTPUT_JSON_BUDGET_ENABLED` is a strict boolean,
default `false`. It applies to both native and retained-provider conversation
transports, and is independent of the relay/backpressure policy in #1688.

## Admission before owned decoding

With the flag enabled, a borrowed Serde visitor validates the runtime Ready
text and each retained ACP payload before bootstrap inspection, session identity
inspection or owned domain decoding. The limits are:

- 4 MiB encoded JSON, preserving the existing retained ACP payload ceiling;
- 8,192 nodes, counting each value, container and object key;
- 64 nested containers, with a root array/object counted as level one.

Duplicate keys consume their full node count, including values that an owned
map might later replace. The visitor borrows strings and does not build arrays,
maps or an owned `serde_json::Value`. Escaped strings still use Serde's temporary
unescape storage; this preflight is not allocation-free. Malformed/trailing JSON
is rejected with a fixed message that includes no payload or parser excerpt.

All thresholds are inclusive. The policy validates the whole document; it never
truncates or partially publishes a frame. Failure is a typed adapter rejection
before the retained cursor is advanced or a `RuntimeOutputReceipt` is queued.
The reader ends the attachment. No discard, parse failure or reconnect invents
an output ACK; the native output remains unacknowledged for explicit recovery.
A payload that always violates the policy cannot be fixed by retrying it, and
must not be represented as successfully consumed.

Enabled attachment requires the runtime's existing retained-output capability
before initialization or prompt submission. An older lossy bridge is refused.
A runtime Open may already have established its process before the Ready check;
the existing opening/retirement lifecycle remains responsible for reconciliation.
The transport's composition setting overrides any renderer-provided preference.

When disabled, the previous decode/receipt behavior remains in place, including
protocol-only receipts for unrecognized or malformed retained payloads. The
flag changes no wire format or persistent schema. Valid terminal correlation,
stream/session identity and durable-consumer ACK ownership remain required.

## Scope and qualification

This limits structural amplification of a single frame. It does **not** prove a
128 MiB process ingress cap or a total RSS limit. Socket input high-water storage,
fragmented WebSocket collectors, concurrent decoders, queued decoded updates,
startup/assembly lifetimes and public serialization need shared accounting.
Tungstenite 0.24 retains input Vec capacity after draining; a per-frame permit
released after parsing would not account for that connection-lifetime storage.

Production activation needs a candidate/config receipt and compatibility
qualification for the supported runtime output shapes, especially rich tool
updates and initialization metadata. Retained frames refused by the new policy
may require a compatible staged rollback or an explicitly approved recovery;
never ACK/drop them just to restore progress. The existing bounded attachment
retry policy is not proof that a persistent refusal will eventually recover.

Local contracts cover exact size/node/depth limits, shallow amplification,
escaped UTF-8, duplicate keys, malformed/trailing data, Ready refusal before
initialization, startup refusal without a receipt, active refusal without an
ACK, valid correlated terminals, transport policy overriding both renderer
preferences, and disabled compatibility. Runs use local
socket fixtures and no model/Sandbox resources.

Do not inject an over-limit frame into the shared production service to prove
this source guard. A later lead-coordinated security-harness compatibility
control may inspect existing authorized output only, with at most two observers,
ten API requests and two minutes, zero new Sandboxes and zero model turns.
Abort on unauthorized data, failed owner control, unexpected 429/503, candidate
drift or health regression. Close bodies and revoke tracked sessions; preserve
the retained-fixture ledger. A larger or destructive payload campaign needs its
own reviewed isolated profile and explicit relay under the existing tier rules.
