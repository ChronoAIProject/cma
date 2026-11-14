# Local EG admission evidence fixture

This fixture prepares the developer-08/developer-15 N1/N2 admission slice for
issue #1966. It is a separate `cmaeg-admission-fixture` executable under
`api/crates/cmaeg/tests/admission_fixture/`; production builds select `--bin cmaeg`.
It is source preparation until its exact source, binary, diagnostic patch,
environment adapter and registered cases have been reviewed and executed. It
provides no production activation or provider capability claim.

The fixture serves generated relay JWKS, a synthetic Responses provider and an
independent reply sink. Provider admission uses the request's exact idempotency
key and body digest. Replay returns the same response; a changed body refuses.
The sink never deduplicates: every authenticated delivery produces a distinct
record. Both operations append and sync their journal before acknowledgement.
A journal failure poisons the fixture; an uncertain write is never silently
retried. Late sink deliveries after closure remain visible. This local provider
contract does not establish idempotency or reconciliation for a production provider.

## Private owner configuration

`EG_DIAGNOSTIC_CONFIG` names a new, regular, private JSON file (0600; at most
32 KiB). Its exact fields are:

- `listen`: local listener address and fixed port; loopback or unspecified bind.
- `advertised_origin`: an explicit loopback HTTP origin visible at the same port
  in each gateway Pod. Product provider/JWKS URL checks remain unchanged. A shared
  remote fixture requires the owner's reviewed per-Pod loopback proxy; an HTTP
  hostname does not satisfy the gateway contract.
- `database`: a fresh `eg_diag_` plus lowercase UUIDv4 hex database.
- `admin_mongodb_uri`, `gateway_mongodb_uri`, `oracle_mongodb_uri`: separate
  owner-supplied local Mongo identities. URI values never enter public receipts.
  Explicit TCP hosts must be private IPs, loopback or the dedicated
  `.cma-scale.svc.cluster.local` namespace. No SRV/public-host fallback exists.
- `private_output`: new absolute directory. The fixture creates it with mode 0700
  and retains custody before migrations. Existing directories are refused.
- `control_bearer`: a runtime generated 32–128-character alphanumeric/hyphen token.
- `replicas_enabled`: Boolean, fixed for the cohort. OFF uses a fresh database.

The environment owner provisions exact schema-derived runtime grants on the
fixture database before invocation. The oracle has its own connection and
identity with the limited runtime role required by the existing record adapter;
its source performs reads only. It does not use the migrator identity for reads.
Broad readWrite/admin runtime credentials fail the product's isolated-grant gate.
The fixture migrates the empty dedicated database, writes one sealed channel,
and writes private `bootstrap.json` containing generated gateway environment,
event/root mapping and cookie-key material. It does not create Kubernetes Secrets
or log/bootstrap secret values. The owner injects the private environment,
listener, Pod IP replica address, cookie-key file and reviewed fixture networking.

## Inner invocation and reconciliation

All `/control/*` requests require the private bearer. One fixture process belongs
to one switch/database cohort. One `POST /control/run` invokes one registered
case with JSON `{case_id, event, endpoints, replays_per_ingress}`: a canonical UUIDv4, a label from
private bootstrap and one or two distinct direct local ingress origins. The replay count defaults to 1 and is bounded to 1 or 2. It sends
one signed event concurrently through the supplied endpoints that many times,
so N1 can exercise two simultaneous replays without inventing a second ingress. Reusing a case ID
is refused before sending; replaying an event intentionally uses a new case ID.
A new case retains the exact signed body for the existing event. Callbacks expire
five minutes after that event was first signed; expiry must not be hidden by
renewing the body of a registered replay case.

The driver reserves a private case receipt before transmission, applies a 25 s
whole HTTP budget per attempt, and continues to the independent durable oracle
after every response, including 503 and transport timeout. The terminal oracle
has a 30 s total budget and 3 s individual read budgets clipped to that deadline.
It reads sealed admission/target/FIFO item/head records, validates identity,
owner, request hash and retention, and retains exact private encrypted/plaintext
receipts. Terminal target state, FIFO completion and provider/sink effects remain
separate observations. A plan alone does not establish completed delivery.

The response has schema `eg_fixture_case_v1`, per-attempt request/endpoint hashes,
case-local monotonic offsets, fixed HTTP code/classification, durable snapshots
and a ledger snapshot. The case digest hashes the canonical UUID’s 16 raw bytes,
matching the diagnostic patch; request digests hash canonical UUID text, matching
the ingress collector. Its status is `OBSERVED_NOT_ACCEPTANCE`. Different case
clocks must not be subtracted. Original same-event healthy behavior still
requires every request to return 202 with the accepted body.

After the outer owner establishes gateway process/receiver finality, call
`POST /control/close`, then `POST /control/reconcile`. The latter performs new
independent reads and writes an immutable `eg_fixture_reconciliation_v1` receipt.
Closure alone is not process death or a no-late-effect proof. `GET /control/ledger`
returns the current sanitized snapshot. The private append-only journal retains
the exact operation sequence and previous-record digest for later independent
verification. A fresh read never rewrites earlier missing/uncertain evidence.

`accounting.py` joins the pre-registered event set, all case attempts, final
oracle, collector records and separately bound outer finality receipt. It
partitions event custody and direct/reconciled/unresolved terminal outcomes.
A 503 with a committed effect remains accountable; it fails a healthy 202 contract.
A pre-submit refusal requires actual fault ancestry for every root invocation,
no later write phase, final absent root/effect and independent finality. Missing
results and rejected log rows hold closure. Counts alone do not establish facts;
`COMPLETE` is structural evidence for independent review, never architect PASS.

## Diagnostic faults and lifetime

Developer15 supplies a separately reviewed generated patch/config for exact
fixture database, canonical root, case and action. No runtime fault endpoint or
production switch is included. One-shot faults require a fresh, bound process;
restarts invalidate the case. The parser retains the actual
`eg_fixture_record_fault` marker, the ordered root-write ancestry and the later
records classification. A marker proves a controlled boundary, not a historical
cause, an external effect or a final HTTP refusal. Successful readback may return 202.

Setup is bounded 60 s. On signal, fixture HTTP shutdown is bounded 60 s, followed by
a 3 s oracle client shutdown. The private journal, final ledger and shutdown
receipt remain. An unmatched started case, failed shutdown or hard process loss
is incomplete evidence. The outer reviewed adapter must enforce its process
watchdog, capture actual gateway/fixture child exits and account for sockets,
resources and transitions. The fixture never deletes its database. Only the
owner may dispose of it after custody/effect/finality evidence is retained.

The separate lifecycle recorder uses prepare, workload, terminal_wait, capture,
workload_shutdown, resource_shutdown, child_wait, local_destroy, cleanup and
transition. Its source tests alone do not prove the historical fixture's shared
post-marker teardown mechanism or controlled stall. Actual success/stall and
source equivalence remain required. The original aadd212 exception and c08
missing phase remain unknowable. N3/scale/HPA and secure early drain retain their
separate assigned owners and gates.
