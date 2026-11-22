# M-SEC-01 runtime isolation cells

This directory turns the five runtime portions of the M-SEC-01 matrix into an executable participant. It expands **30 cells**: network, file, metadata, Kubernetes and credential isolation × cold, warm and resumed × tenant A→B and B→A. `am-a` and `am-b` are ingress processes; `tenant-a` and `tenant-b` are independent security principals.

`runSec01()` is called by the sole local two-AM parent. It never starts a process, launches a browser, restarts an AM or changes HPA. The parent injects the hooks in `fixtures.json`, a pinned AM/runtime candidate (`source`, `image`, `runtimeSource`, `runtimeImage`), a disposable parent lease and a private evidence recorder. A missing hook produces a retained `BLOCKED` cell for every affected target. The runner keeps every probe outcome and reports per-cell counts; a rerun does not erase an earlier failure.

The runtime primitive in `probe.py` performs only bounded TCP and synthetic-file observations inside an authorized tenant runtime. It emits hashed addresses and bounded byte counts, never paths, file content, tokens or environment. The parent must join it to independent policy and owner-side canary evidence. `kubectl exec`, host filesystem inspection and a fixture boolean are not valid substitutes. Each probe runs against both AM ingresses and carries a real serving receipt.

A cell can pass only when all required positive controls work, all prohibited probes are denied with correlated policy evidence, both AM serving bindings are independently observed with Pod/process/container/origin identity, and the lifecycle state is proved:

- `cold`: new runtime and no tenant history;
- `warm`: actual post-retirement pool reuse with sanitization;
- `resumed`: persisted runtime paused, authorization changed while paused, then restored through the normal product path.

The local result is rehearsal evidence. It cannot close production M-SEC-01, which also requires the matrix's route, revoke/export, receipt, identity-negative and scan obligations.

Run the offline contract checks from the repository root:

```sh
node --test .github/scripts/multi-am-features/sec01/*.test.mjs
python3 -c "from pathlib import Path; p=Path('.github/scripts/multi-am-features/sec01/probe.py'); compile(p.read_text(), str(p), 'exec')"
```

A parent handoff should retain the private result packet and publish only its SHA-256 and the `by_category`/`counts` objects. The runner removes unrecognized observation fields, including raw body, path and authorization fields, before evidence is written. Adapters must keep the retained fields free of credentials and private payloads; the field filter cannot detect sensitive data hidden under an allowed key.

Fixtures must declare `evidence_kind: 'OFFLINE_SYNTHETIC'` or
`evidence_kind: 'RUNTIME_OBSERVATIONS'`. Every recorded event, cell result and run
packet retains that declaration. Synthetic packets use scope
`OFFLINE_CONTRACT_CHECK`; runtime packets use `LOCAL_RUNTIME_REHEARSAL`.
An absent or unknown declaration blocks every cell before setup. The declaration
identifies the adapter's evidence source; it neither authorizes execution nor
proves that a runtime ran. The included test adapter declares synthetic evidence.
HTTP and credential refusals require the same independent target-canary receipt
as file and TCP refusals. A typed denial without that receipt remains BLOCKED.

`binding.mjs` owns the minimal pure serving-receipt validator. It requires the
same ready, non-draining AM process, Pod, container and loopback origin before
and after the request, a request ID, ordered observation times, and explicit
`pod_forward_before_after` attribution. It imports no parent launcher.

Offline fixtures validate this contract only. A current adapter and separately
authorized runtime execution lane are still required; missing hooks or receipts
keep all affected cells BLOCKED. Neither these unit tests nor merging the harness
qualifies the runtime campaign or closes #2213 or #1549.

TCP specifications carry `required_family` (`ipv4`, `ipv6` or `dns`). Adapters
must pass it as the primitive's `family` and retain its `requested_family`, each
observed `family`, and portable `failure_class`. Numeric errno alone is not
denial evidence. The producing OS classifies permission/connection refusal,
timeout, protocol/address unavailability and other errors; only `refused` can
join independent policy evidence. Both IPv6 scenarios require actual IPv6
observations. Missing/mismatched families, timeouts and unavailable protocols
remain BLOCKED. Any demonstrated prohibited connection remains FAIL, even if
another address or attribution record is incomplete.

File observations carry a separate `failure_class` on each operation. The
producing OS classifies EACCES and EPERM as `permission_denied`, ENOENT as
`not_found`, timeouts as `timeout`, unavailable storage as `unavailable`, and
other failures as `error`. Only `permission_denied` qualifies with the existing
canary and correlated policy evidence. A `not_found` operation additionally
requires `policy.basis: 'isolated_namespace'`: that policy receipt must establish
that the target exists for its owner but is absent from the probing tenant's
namespace because of isolation enforcement. Missing paths alone prove nothing.
Every failed operation requires a positive integer errno. Missing or malformed
classifications, zero errno, timeouts and I/O errors remain BLOCKED. Any
successful prohibited operation remains FAIL despite other errors or missing
evidence.

All HTTP negatives in this matrix forbid a downstream connection:
`redirect_destination`, `proxy_direct` and `metadata_id`. Their observations
require a measured Boolean `destination_connected: false`, bound to the same
probe and serving receipt. A typed HTTP refusal with zero private bytes does
not establish that absence. Missing, null or wrong-type measurements remain
BLOCKED; a measured connection remains FAIL even without other prerequisites.
Functional positive controls still exercise their allowed destinations.
