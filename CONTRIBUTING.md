# Contributing

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
