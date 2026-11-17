# M-SEC-01 runtime isolation cells

This directory turns the five runtime portions of solution-architect-05's M-SEC-01 matrix into an executable participant. It expands **30 cells**: network, file, metadata, Kubernetes and credential isolation × cold, warm and resumed × tenant A→B and B→A. `am-a` and `am-b` are ingress processes; `tenant-a` and `tenant-b` are independent security principals.

`runSec01()` is called by the sole local two-AM parent. It never starts a process, launches a browser, restarts an AM or changes HPA. The parent injects the hooks in `fixtures.json`, a pinned AM/runtime candidate (`source`, `image`, `runtimeSource`, `runtimeImage`), a disposable parent lease and a private evidence recorder. A missing hook produces a retained `BLOCKED` cell for every affected target. The runner keeps every probe outcome and reports per-cell counts; a rerun does not erase an earlier failure.

The runtime primitive in `probe.py` performs only bounded TCP and synthetic-file observations inside an authorized tenant runtime. It emits hashed addresses and bounded byte counts, never paths, file content, tokens or environment. The parent must join it to independent policy and owner-side canary evidence. `kubectl exec`, host filesystem inspection and a fixture boolean are not valid substitutes. Each probe runs against both AM ingresses and carries a real serving receipt.

A cell can pass only when all required positive controls work, all prohibited probes are denied with correlated policy evidence, both AM serving bindings are independently observed with Pod/process/container/origin identity, and the lifecycle state is proved:

- `cold`: new runtime and no tenant history;
- `warm`: actual post-retirement pool reuse with sanitization;
- `resumed`: persisted runtime paused, authorization changed while paused, then restored through the normal product path.

The local result is rehearsal evidence. It cannot close production M-SEC-01, which also requires the matrix's route, revoke/export, receipt, identity-negative and scan obligations.

Run the offline contract checks from the repository root:

```sh
node --test .github/scripts/multi-am-features/sec01/runner.test.mjs
python3 -m py_compile .github/scripts/multi-am-features/sec01/probe.py
```

A parent handoff should retain the private result packet and publish only its SHA-256 and the `by_category`/`counts` objects. The runner sanitizes retained observations and rejects raw paths, bodies, cookies, authorization values and token-like fields before evidence is written.
