# Independent parent evidence contract, version 2

The parent is the trust boundary, as in C275's existing live topology and lease
observers. Registrar output cannot select an issuer, public key, release,
expected subject or expected session. Production NyxID signup is not authorized.
A real nonproduction deployment and its approved response mapping are prerequisites.

`trust` is parent-frozen `{key_id, public_key_pem, environment: "nonproduction",
issuer, response_schema}`. The key is Ed25519. The issuer is the approved HTTPS
NyxID issuer. `response_schema.registration` and `.verification` each contain
JSON pointers `{subject, registration_id, created_at}` for the actual provider
JSON response. These pointers must be reviewed against that NyxID deployment's
schema; the suite does not guess a registration API or flatten provider evidence.

The signing key is held by an independent capture process, never the registering
adapter, fixture owner or suite. It signs only its own observed provider exchanges
and browser callback. A parent that grants the registrar this key invalidates the
proof. An envelope is `{key_id, payload, sha256, signature}` where `sha256` is the
SHA-256 of recursively sorted-key canonical JSON `payload` (arrays preserve order),
and `signature` is Base64 Ed25519 over those same bytes. `provenance.mjs` implements
and verifies this exact encoding. The envelope and hashes are retained in private
case evidence; raw credentials, cookies and provider bodies are not logged.

## Registration fixtures

`fresh-identity`:

- `register({challenge, purpose: "M-FUNC-01", issuer, local_kind: true, signal})`
  performs one real fresh signup/login and binds a new transport actor. Returns
  only `{actor}`; claimed freshness flags are ignored.
- `cleanup({challenge, mode})` reconciles even a lost signup response. Mode is
  `delete_account` after scoped resources are released, or
  `revoke_session_retain_account` when resource reconciliation remains unresolved.

A distinct `registration-evidence` fixture:

- `collect({challenge, signal})` obtains the independent signed envelope.
- `readCapture({sha256, signal})` returns immutable captured provider body bytes
  (string or Buffer), privately. The suite verifies their hashes and extracted
  values itself. Missing bytes or a mismatched artifact blocks acceptance.

Registration payload fields:

```text
schema: mfunc01-nyxid-registration-v2
challenge: exact nonce supplied before signup
registration:
  issuer, method: POST, status: 200 or 201
  subject_sha256, registration_id, request_id, created_at, response_sha256
verification:
  issuer, method: GET, status: 200
  subject_sha256, registration_id, request_id, created_at, observed_at, response_sha256
session:
  authentication: browser_session
  subject_sha256, origin, cookie_jar_sha256, callback_sha256, binding_sha256
```

The verification GET is a **separate authenticated provider read by the capture
process**, with its own request ID. Both provider body artifacts must independently
resolve to the same subject, registration ID and creation time. Creation must be
inside this invocation's registration interval and observation after creation.
The callback artifact must also exist and match its hash. The suite refuses old
accounts rather than trusting an empty inventory as proof of fresh registration.
Timestamp precision and clock agreement must be validated by the real adapter;
this contract does not silently widen the freshness window.

`cookie_jar_sha256` hashes canonical JSON of the actual scoped cookie array,
projected to `{name,value,domain,path,httpOnly,secure}` and sorted by canonical row.
At least one populated HttpOnly cookie is required. `binding_sha256` hashes
canonical `{subject_sha256,cookie_jar_sha256,origin}`. Only digests leave this
operation. J0, J2 and J7 independently read `/auth/session` using that actual
Playwright cookie context and verify this binding; J2/J7 require
`authentication: browser_session` in their sanitized browser evidence.

## Mutation evidence

Normal existing-Agent writes consume the C275 runner's `owner_proof` events.
`invokeJourney` captures those events privately; no contributor-provided
`ctx.runState.ownerProofs` is accepted. Every write receipt must find exactly one
proof matching request ID, Agent digest, actual ingress and positive owner epoch.
A proof for one write cannot stand in for another write or a later read.

`lifecycle-evidence.collect({request_id, signal})` supplies a signed envelope for
a creation or terminal transition, under independently frozen `lifecycleTrust`.
Its payload has:

```text
schema: mfunc01-lifecycle-owner-v1
challenge: request_id
kind: lifecycle
method, path_sha256, status, request_id, agent_sha256
owner: a or b; ingress: a or b; owner_epoch: positive integer
owner_instance_sha256, ingress_instance_sha256
operation_sha256: lifecycle operation ID hash (required for Agent create)
verdict: PASS (accepted only after all joins and signature are verified)
```

The producer must derive these fields from the actual request, lifecycle/worker
claim and committed effect captures. It cannot sign a later arbitrary owner
snapshot or a desired outcome. Hashes of raw witness captures and their immutable
storage are the producer's retention responsibility. A status response alone is
insufficient. Missing terminal ownership/lineage fields block the journey and
must be reported to the parent; there is no synthetic terminal-lease fallback.
