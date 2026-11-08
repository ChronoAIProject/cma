# Request dependency failure attribution

CMA emits one `request dependency unavailable` warning for each completed 503
at the ambient credential-resolution boundary, Agent file-content read, or Agent
model-list read. The public status, body, headers, authorization checks, retry
policy and deadlines are unchanged. An authentication failure ends the request
before either read handler, so it is counted only as `authentication`.

The API uses structured log metrics. These events carry
`metric="cma_dependency_failures_total"`, `operation`, `dependency`, `reason`,
`count=1` and `failures_total`. The last field is a saturating process-local
counter for that exact category tuple; it resets on API process restart. Count
events (or sum `count`) for a time-window total. Do not sum successive cumulative
values or merge cumulative series from separate processes without reset handling.
No new exporter, endpoint, configuration, or infrastructure is required.

Operations are the fixed labels `authentication`, `file_content`, and
`model_list`. Dependency and reason labels come from closed enums. Events have
no parent span, request path, identifier, credential, upstream URL, raw error,
model name, or response content. Unknown attribution is explicit; error text is
never parsed to guess a dependency. A future-local slot preserves the first
known final failure before public error projection. It adds no task, retry,
queue, network call, lock, or change to request scheduling. Nested and concurrent
captures are isolated. A cancelled future emits nothing.

| Dependency | Meaning |
| --- | --- |
| `identity_provider` | Identity verification or organization-membership dependency (NyxID in the production composition). |
| `storage` | Lineage/resource access persistence or credential custody read/validation. |
| `runtime` | File read through the runtime adapter. |
| `model_gateway` | Model catalogue request through the configured NyxID model proxy. A returned 503 does not prove which downstream provider failed. |
| `authentication_authority` | Local credential delivery, scope, expiry or custody mismatch; resource authority requiring revalidation; or an otherwise unattributed authentication authority error. |
| `local` | Invalid composition/configuration or an unattributed final 503. |

Identity failures retain the verifier's existing final reason after single-flight
fan-out: `timeout`, `connection`, `http_502`, `http_503`, `invalid_evidence` or
`verification_deadline` (the verifier's `deadline` category). `invalid_evidence`
retains the existing broad non-transient classification, including malformed
provider evidence, local capacity and outer verification deadlines. It does not
assert that a provider returned malformed data. This change does not make any of
these cases eligible for retry or cached fallback. Successful fallback, successful
verification, credential rejection and recovered transient attempts do not count
as final dependency failures. Unclassified auth-store/custody errors remain
`authentication_authority/unknown` instead of being guessed from a message.

File reads distinguish persistence (`unavailable`, `corrupt`), membership
(`unavailable`, `invalid_evidence`), authority (`authority_changed`) and runtime
(`unavailable`, `capacity`, `runtime_ledger_hold`, `input_authority`,
`prompt_not_written`). Existing 400/403/404/409/413/429/504 outcomes do not increment
the 503 counter. In particular, runtime-not-ready and unsupported-file responses
keep their existing 409 behavior.

Model-list reads distinguish the same lineage/access dependencies plus local
`configuration`; credential `credential_missing`, `credential_expired`,
`credential_mismatch`, `scope_missing` and `invalid_evidence`; and gateway
`timeout`, `connection`, `transport`, `http_502`, `http_503`, `http_429`,
`http_other`, `body_timeout`, `response_body`, `response_too_large` and
`invalid_evidence`. Catalogue parse failures do not expose model names or body
text. Runtime capability observation can make models unavailable for selection;
that successful response is not a dependency-failure event.
