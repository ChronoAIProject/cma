# Community drafts: pending-decision checklist

All items below are **pending**. This checklist maps the community-file
placeholders to decisions and acceptance evidence. It does not adopt a license,
contribution agreement, support promise, reporting channel or enforcement policy.
Keep approval records, rights documents, test reports and personal contact details
in private custody; publish only approved policy text and intended public contacts.

Check an item only after the responsible owner confirms the decision and the
affected drafts agree. A policy decision does not by itself authorize repository
settings changes, publication, asset removal or release operations.

## Project and contribution terms

- [ ] **C01 — License and copyright authority.** Confirm the project license,
  covered first-party scope, rights holder and review of contribution rights.
  Record the approved identifier or exact terms and distinguish third-party
  material. Update the license/authority placeholders in
  [CONTRIBUTING](../CONTRIBUTING.md) and the
  [PR template](PULL_REQUEST_TEMPLATE.md). A later authorized license/metadata
  change must agree with that decision; this checklist does not create a grant.
- [ ] **C02 — DCO or CLA.** Choose the contribution agreement and approve its
  exact text, contributor instructions, verification process and responsible
  reviewers. Specify how compliance is checked before accepting a contribution.
  Align CONTRIBUTING, the PR template and the
  [feature template](ISSUE_TEMPLATE/feature_request.md). Do not require a sign-off
  or claim a CLA is accepted before the chosen process is published and usable.
- [ ] **C03 — ReactDrops disposition.** Choose independent replacement, explicit
  written redistribution permission or a defined exclusion. Specify the covered
  source and release artifacts, including derived SVGs, and separately decide
  historical distribution handling. Align CONTRIBUTING, the PR template and the
  [material inventory](scripts/license-inventory/REACTDROPS.md). If replacement is
  selected, approve static presentation or the optional reveal in the
  [behavior specification](scripts/license-inventory/REACTDROPS-REPLACEMENT.md),
  including its intentional behavior changes. No asset disposition is implied
  by a disabled feature flag.
- [ ] **C04 — Release and support scope.** Identify source-only or the exact
  browser/binary/image distributions, supported versions, support duration,
  environments and responsibility for hosted versus self-hosted use. Distinguish
  release inclusion from a support commitment. Align CONTRIBUTING,
  [SECURITY](SECURITY.md), the PR template and both
  [bug](ISSUE_TEMPLATE/bug_report.md) and feature templates. Review the actual
  artifacts and applicable third-party obligations before making release claims.
- [ ] **C05 — Review responsibility.** Confirm public maintainer roles for
  ordinary changes and designated CI/security review. Approve the proposed
  non-author review requirement and check the effective repository rules after
  any separately authorized settings changes. Align CONTRIBUTING and the PR
  template; distinguish a documented expectation from an enforced branch rule.

## Private security reporting and response

- [ ] **S01 — Primary reporting channel.** Approve the primary private reporting
  URL. If GitHub private vulnerability reporting is selected, an authorized
  maintainer must enable it and verify access and receipt with a harmless,
  authorized test from an ordinary reporter's perspective. Confirm that the
  intended triage role receives it privately. Replace SECURITY's proposed-channel
  placeholder only after this succeeds; adding SECURITY alone enables nothing.
- [ ] **S02 — Private fallback.** Approve a monitored fallback URL and verify that
  a reporter can use it when the primary route is unavailable. Publish only the
  intended public contact endpoint. Remove the fallback placeholder and retain
  instructions to withhold sensitive details from public issues.
- [ ] **S03 — Security responsibility.** Confirm the public primary and backup
  roles, their report access and who covers absence or handover. Resolve SECURITY's
  ownership placeholder and the monitored-owner condition in S01 consistently.
- [ ] **S04 — Acknowledgement target.** Approve a realistic response window,
  including its clock basis and coverage expectations. Replace SECURITY's
  acknowledgement placeholder; do not imply a response SLA before approval.
- [ ] **S05 — Progress updates.** Approve an update cadence and responsibility for
  maintaining contact while investigation or remediation remains open. Resolve
  SECURITY's progress placeholder consistently with S03 and S04.
- [ ] **S06 — Coordinated disclosure.** Approve fix coordination, advisory
  publication, reporter credit preferences and a private escalation path. State
  who decides disclosure timing and how unresolved cases are handled. Replace
  SECURITY's disclosure placeholder without exposing private incident records.

SECURITY's version/support placeholder is covered by C04. The public issue
templates must continue directing vulnerability details away from public intake;
the references in CONTRIBUTING and the conduct policy must lead to the adopted
security instructions.

## Conduct policy and private reporting

- [ ] **K01 — Adoption and scope.** Approve the conduct policy's participation
  standards, covered project spaces, enforcement authority and available
  responses. Resolve the draft/adoption statements in
  [CODE_OF_CONDUCT](CODE_OF_CONDUCT.md) only when the process below is ready.
- [ ] **K02 — Private conduct channel.** Approve and test the private reporting
  URL with a harmless, authorized test; confirm receipt by the intended role.
  The conduct channel must be appropriate for sensitive personal reports and
  remain distinct in purpose from vulnerability reporting.
- [ ] **K03 — Responsible maintainers.** Confirm public primary and backup roles,
  report access and responsibility for responding. Resolve the conduct policy's
  ownership placeholder; do not invent an enforcement team or personal contacts.
- [ ] **K04 — Conflicts and escalation.** Approve an independent reviewer role and
  private route for complaints involving a responsible maintainer. Resolve the
  conflict/escalation placeholder and document when a reviewer must step aside.
- [ ] **K05 — Acknowledgement and appeals.** Approve response windows, the appeal
  procedure, who reviews an appeal and how the outcome is communicated. Resolve
  the conduct policy's acknowledgement/appeal placeholder. Keep report access
  limited to people needed for the process.

## File-by-file reconciliation

| Draft | Pending decisions to resolve |
| --- | --- |
| [CONTRIBUTING](../CONTRIBUTING.md) | C01–C05; community onboarding status; references to the adopted security and conduct policies. |
| [SECURITY](SECURITY.md) | S01–S06 and C04; verified primary/fallback channels; matching ownership, response and support statements. |
| [CODE_OF_CONDUCT](CODE_OF_CONDUCT.md) | K01–K05; private reporting, independent escalation and the adopted security reference. |
| [PR template](PULL_REQUEST_TEMPLATE.md) | C01–C05; contribution terms, restricted material, release scope and review expectations. Preserve existing validation and migration requirements. |
| [Bug template](ISSUE_TEMPLATE/bug_report.md) | C04 and C05; support/intake wording and a valid reference to private vulnerability reporting. |
| [Feature template](ISSUE_TEMPLATE/feature_request.md) | C01, C02 and C04; contribution/release wording and accurate third-party provenance instructions. |

## Before changing a draft to an adopted policy

- [ ] Replace each applicable `[PENDING: ...]` with approved, consistent text.
  Keep unresolved files visibly in draft; deleting a marker is not a decision.
- [ ] Verify private reporting routes and responsible coverage before presenting
  either reporting policy as active. Store test details privately.
- [ ] Check links, issue-template rendering and contribution instructions from
  an external contributor's perspective, without submitting sensitive reports.
- [ ] Review names, contacts, examples and attachments for the public repository
  boundary; include no credentials, internal systems or private approval records.
- [ ] Reconcile license and release claims with actual material disposition and
  applicable third-party obligations. A notices draft is not release clearance.
- [ ] Record which policy text was approved and obtain the required review of
  the resulting change before the separately authorized integration/publication.

None of these decisions changes the existing engineering, compatibility or
release-safety requirements. This checklist adds no DCO/CLA gate, settings change,
support commitment or automatic publication action.
