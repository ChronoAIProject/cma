# Reject invalid native identities before copying

The fresh `session/new` reply previously copied `sessionId` from JSON and cloned
that string for activation before enforcing the existing identity rule. A large
invalid identity could therefore allocate independent copies before rejection.

With `CHRONO_RUNTIME_JSON_RETENTION_BOUNDS_ENABLED=true`, the fresh-reply path
validates the borrowed string before copying it. The existing rule requires a
nonempty identifier of at most 1,024 UTF-8 bytes without control characters.
Accepted values, Unicode byte semantics, missing/wrong-type errors and rejection
messages remain unchanged. The disabled path retains its original projection;
activation validates fresh and resumed values through the same common guard.
An early refusal still occurs within the existing `session_new` startup phase.

The policy is false by default and stays OFF on Thursday's train. This change
adds no pool or configuration value. It removes rejected-input copies; valid
active identities, source receipts, ACK queue copies and durable correlation
storage still require aggregate ownership accounting under #1702.

## Qualification

Contracts cover accepted ASCII/Unicode byte boundaries, empty, oversized and
control-bearing identities, wrong/missing types and unchanged fixed errors.
Retained startup tests verify exact successful activation and ordered receipts,
and verify refusal without startup effects or fabricated ACK. The common final
activation guard still rejects invalid or changed identities with either policy.
A maximal admitted reply measures fresh projection plus identity validation;
the JSON tree and additional activation clones are outside that measurement.
Local qualification passed 677 focused tests, workspace formatting, strict
all-target/all-feature CMA/core/adapter clippy, source-size, configuration/docs,
whitespace and secret-history checks. All changed Rust files are below 500 lines.

Qualified source and API tree, using Rust 1.96.1 on aarch64-apple-darwin:

```text
revision 04eaf9d48250fc33d5ff91f00a459f1260ce5685
tree df971de007773a7182c0619a0cf470caf6a1e10e
```

The fixture encodes to 4,194,304 bytes and selects a 4,194,266-byte identity.
Projection followed by common validation peaks at 4,194,316 requested bytes
on the legacy path and 50 bytes with the policy enabled. The latter is the
fixed rejection message. Both return to zero tracked bytes. This measures
the selected projection, not the entire handshake or process RSS.

Rollback follows the existing JSON-retention policy for new attachments. No
production activation, live traffic or Sandbox/model work occurs in this slice.
Full typed/container, correlation, codec, desktop, durable working-storage and
aggregate/RSS acceptance remain open.

Part of #1472. Implementation child #1736 under #1702.
