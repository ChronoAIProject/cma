# Contributing

The first OSS release covers first-party source under [Apache-2.0](LICENSE).
Contributions use the Developer Certificate of Origin below. Third-party material
retains its own terms. The engineering requirements below remain applicable.

## Release and contribution scope

| Topic | Decision/status |
| --- | --- |
| Project license and copyright authority | Apache License 2.0; first-party source scope is covered by `LICENSE` and `NOTICE`; third-party material keeps its own terms. |
| Contribution agreement | Developer Certificate of Origin 1.1; contributors sign off each commit as described below. |
| Restricted third-party materials | The restricted decoration and derived SVGs were removed from the source tree. No replacement is included. |
| Release and support scope | The first OSS release is source-only. Binaries, images and hosted-service support are outside this release. |
| Review responsibility | **[PENDING: public maintainer roles for ordinary, CI and security-sensitive changes]** |

The [community decision checklist](.github/COMMUNITY_DECISIONS.md) records the
remaining governance decisions and their acceptance evidence.

Do not infer reuse or redistribution permission from the repository being public.
Do not apply the project license to third-party material. Preserve each
third-party copyright, license and NOTICE requirement.

## Developer Certificate of Origin

Contributions use the [Developer Certificate of Origin 1.1](https://developercertificate.org/).
By adding a `Signed-off-by` trailer, the contributor certifies that they have the
right to submit the work under the project's terms and that they understand the
contribution is provided under the Apache License 2.0. Add the trailer to every
commit in the contribution:

```text
Signed-off-by: Your Name <your-public-account-address>
```

Use a public account address that is appropriate to disclose in commit history;
do not put private contact details, credentials or personal data in commits.
The maintainer review process checks the trailer before accepting a contribution.

## Proposing a change

Search existing issues first and agree on a focused issue before implementation.
Keep each change reviewable, describe the user-visible result, add relevant tests,
and report checks actually run, including failures and untested paths. Link the
PR to its issue; use a closing keyword only when its full acceptance scope is met.
Use synthetic examples and remove credentials, personal data and private system
details from code, logs, screenshots and issue text.

For vulnerabilities, read the [security reporting policy](.github/SECURITY.md)
before sharing any technical detail. Community participation expectations remain
in the [code of conduct draft](.github/CODE_OF_CONDUCT.md).

When you add or update a dependency, commit any resulting lockfile changes and
review the licenses of the resolved packages. For copied code, media, or fonts,
record the upstream revision, the files you copied, and any changes you made.
Update the vendor catalog and keep the applicable upstream copyright, license,
and NOTICE text. Run the source license inventory and include any missing terms
or collection errors in your PR. Keep generated drafts in private storage.
[Source license inventory](.github/scripts/license-inventory/README.md)

Open your PR against `develop`. Request a review from a maintainer other than
the author. For changes to CI or security-sensitive code, request review from
the responsible maintainer. [Community decisions](.github/COMMUNITY_DECISIONS.md)

## Development and review

Read `CLAUDE.md` for architecture, compatibility and review requirements. Link a
focused pull request to its tracking issue and explain the behavior and checks.
Run the relevant Rust checks from `api/` and web checks from `web/` using the
pinned toolchains and lockfiles. Run the public repository boundary regression
suite with `ruby .github/scripts/check-public-tree-tests.rb`.

## Data migrations

For each selected release batch, inventory affected stores, formats and every
writer: requests, workers, startup/import, retries, recovery, runtime and home
files, client journals and external effects. Include indexes, validators, TTL
and other startup DDL. Name an owner and required evidence for each step.
Assign versioned step IDs and checksums, declare phase prerequisites and ordering,
and require scoped authority, fencing or compare-and-swap, idempotency, bounded
work and durable progress and exception receipts. Test interruption and resume.

Follow expand → compatible readers → gated writers → bounded backfill → contract:

1. **Expand.** After inventory review and dry-run/preflight, add compatible
   formats or structures without removing retained data or existing access.
   Before any irreversible change, require a coherent, independent, recoverable
   backup, restore evidence and a named restore owner. Account for post-backup
   writes. Define the exact compatible forward-repair procedure, permitted
   recovery artifact and abort criteria. Preserve IDs, histories, newer fields,
   operation identity, homes, runtime pins, assets and key dependencies.
   Destructive database rollback or dropping fields to fit an older binary is
   not a recovery plan. Startup must validate declared supported state. Explicitly
   inventory and justify any retained startup DDL; do not hide migration there.
   A lease cannot undo DDL already submitted.
2. **Compatible readers.** Once expanded state is ready, qualify and deploy
   readers before admitting new formats. Record exact immutable compatible
   image, executable and asset identities, including permitted recovery, for
   the entire selected release batch. Execute those artifacts against retained
   and new formats; decoder tests alone do not qualify them. Prove incompatible
   readers refuse before traffic and writes. Retain compatibility until old
   readers and writers are drained and approved recovery remains possible.
3. **Gated writers.** After reader and recovery qualification, authorize only the
   selected writer cohort with verified authority and process-drain evidence.
   Keep new writers default-off until that authorization. Verify disabled and
   enabled behavior. A default-off flag protects new admission, not readability
   of records already promoted. Disabling it must retain existing reads, replay,
   acknowledgements and recovery, and cannot lower a durable reader floor.
4. **Bounded backfill.** After the reader and writer prerequisites pass, execute
   the declared steps within reviewed limits. Record durable progress and
   exceptions, reconcile outcomes against the selected cohort, and prove safe
   retry, interruption, resume and takeover. Preserve recovery throughout;
   completion requires the declared criteria and evidence, not merely a process
   exiting successfully.
5. **Contract.** Make a separate, explicit later decision after backfill evidence
   is accepted, exceptions are resolved, and old readers and writers are drained.
   Prove approved recovery remains possible before removing compatibility.
   Merge does not authorize automatic cleanup.

Record verification and adoption in the actual release record. Bind results to
source SHA, immutable artifact identities, configuration, environment, time
window, sample counts and bounds. Include failed or unexecuted matrix cells,
raw evidence references, exception status and a named verification and closure
owner. Keep operational records and raw evidence in approved private custody;
public PRs contain scrubbed descriptions and authorized references only. A green
PR is neither production verification nor permission to activate writers. Use
`Refs` for partial work; issue closure requires full acceptance and required
release evidence.

## Release safety requirements

Release qualification must cover transitions into protected operation, as well
as steady-state behavior. Record adoption of the following requirements in the
private release and on-call records; source review or a bounded model pass does
not establish deployed compliance.

- CMA's own model budget (`model_budget` / `BudgetSettings`) stays DISABLED at launch; spending limits and charging are NyxID's responsibility.
  CMA produces model and Sandbox usage; NyxID prices and charges.
  If CMA's model budget is ever enabled later, F1 applies: settle in-flight unbudgeted work first.
- **Ownership clocks.** The receiver lease lasts 10 seconds from challenge
  creation; the ensuing Mongo owner renewal expires after 15 seconds. At every
  expiry-based takeover, the receiver's earlier deadline must already have
  elapsed. At equal elapsed-clock rates, Mongo's cumulative positive clock lead
  over the receiver must be at most 5 seconds, with an explicit operational
  uncertainty margin below that bound. Repeated adjustments share this budget;
  it is not a per-adjustment allowance or a static wall-clock offset check.
  Qualify drift and suspend/resume behavior for every participating receiver
  path. A receiver must account for suspended duration or refuse old-session
  effects on resume until a fresh lease is established. Unknown clock behavior
  blocks ownership rollout qualification. See [#1541](https://github.com/ChronoAIProject/cma/issues/1541).

Bind evidence to the actual candidate, effective configuration and accounting
policy. Assign verification and on-call owners, preserve failed or unknown
results, and record hold and recovery criteria. Keep runbooks, raw observations,
account identifiers and operational approval records in private custody.

## Public repository boundary

Never commit infrastructure or deployment manifests, Kubernetes or Helm YAML,
cloud project, cluster, IAM or service-account identifiers, internal hostnames
or IPs, Terraform state, or environment-specific configuration. Infrastructure
lives outside GitHub. Internal documentation, runbooks, verification records,
notes, plans and investigations are not committed to this public repository.
Do not add `docs/`, `infra/`, or `sdk/`, or copy private material elsewhere in the
tree. Coordinate operational changes through private maintainer channels.

The three public Event Gateway guides are code-owned application assets. Update
their scrubbed examples and contract tests together. Keep environment identifiers
and credentials out of examples, test fixtures, issues and pull request text.
