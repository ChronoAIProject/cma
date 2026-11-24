# Community policy decision record

This file records the owner decisions applied to the first source-only OSS
release and the governance decisions that remain open. It is a coordination
record, not a license grant or a repository-settings change. Keep approval
evidence and personal contact details in private custody.

## Applied decisions

- [x] **C01 — Project license.** First-party source is released under the Apache
  License 2.0 in `LICENSE`, with the project notice in `NOTICE`. Third-party
  components retain their own terms and are tracked by the source inventory.
- [x] **C02 — Contribution agreement.** Contributions use the Developer
  Certificate of Origin 1.1. `CONTRIBUTING.md` requires a `Signed-off-by` trailer
  on every contributed commit; no CLA is required for this release.
- [x] **C03 — Restricted effect disposition.** The restricted effect,
  its derived SVG assets and their integration/build wiring were removed. No
  replacement is included in this release. The source tree and source-only
  distribution must remain free of those files and references.
- [x] **C04 — First release scope.** The first OSS release is source-only. It does
  not publish binaries, images or a hosted-service support commitment.
- [x] **S01 — Vulnerability channel.** Use GitHub private vulnerability reporting;
  no personal email channel is published. The maintainers named for reports are
  `@chronoai-shining` and the repository owner.

## Remaining decisions

- [ ] **C05 — Review responsibility.** Confirm public maintainer roles for ordinary
  changes and designated CI/security review. The PR template documents the
  expectation but does not assert an effective branch rule.
- [ ] **K01 — Code-of-conduct adoption.** Approve the participation standards,
  covered spaces, enforcement authority and available responses before changing
  `CODE_OF_CONDUCT.md` from draft to active policy.
- [ ] **K02 — Conduct reporting channel.** Approve and test a private channel for
  sensitive conduct reports; it remains distinct from vulnerability reporting.
- [ ] **K03 — Conduct responsibility.** Confirm public primary and backup roles for
  conduct reports and their limited-access handling.
- [ ] **K04 — Conduct conflicts.** Approve an independent reviewer and private
  escalation route for reports involving a responsible maintainer.
- [ ] **K05 — Conduct acknowledgement and appeals.** Approve response windows,
  appeal procedure and the reviewer responsible for appeals.
- [ ] **S02 — Security response practice.** The security policy deliberately
  promises no response-time SLA. If an acknowledgement target, update cadence or
  advisory process becomes a commitment, approve and update `SECURITY.md`.

## Reconciliation checklist

- [ ] Replace only the remaining placeholders after the corresponding decision is
  approved; do not silently turn an expectation into an enforced setting.
- [ ] Verify GitHub private vulnerability reporting access through an authorized,
  harmless test before calling the channel operational. Store test evidence
  privately.
- [ ] Check policy links and issue-template rendering from an external
  contributor's perspective. Do not submit sensitive reports as a test.
- [ ] Keep credentials, personal data, private approval records and internal
  infrastructure details out of public policy files.
- [ ] Reconcile the Apache-2.0 notice and source inventory with the actual
  source-only tree before each source release.

The applied decisions above are reflected in `LICENSE`, `NOTICE`,
`CONTRIBUTING.md`, `SECURITY.md`, the PR/issue templates and the tracked source
tree. Remaining decisions do not block the stated first-release license or
source-only scope, but they must be resolved before promising additional
community governance or support commitments.
