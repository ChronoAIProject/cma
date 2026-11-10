# Migration reader matrix evidence

This is the first CI tooling slice of [#1327](https://github.com/ChronoAIProject/cma/issues/1327),
under the [data migration standard](CMA_Data_Migrations.md). The tool validates
the consistency and completeness of a reviewed plan and retained execution
receipts. It does not run service images, select a recovery image, inspect
production or certify M-OPS-02. The CI job runs synthetic success/failure tests
of the validator; its green status is not a completed production reader matrix.

## Reviewed input and verification

The release owner reviews the plan independently of its execution evidence.
Each plan covers one service's complete selected reader set and durable fixture
cohort. Store sensitive fixtures/results outside the checkout with restricted
access. Never put production record identities, message bodies or credentials
in the plan, receipt or GitHub summary. Synthetic fixture IDs are sufficient.

Run from the repository root, supplying the independently selected candidate:

```sh
node .github/scripts/migration-matrix.mjs \
  /protected/campaign/plan.json /protected/campaign/evidence.json \
  "$CANDIDATE_SOURCE_SHA" "$CANDIDATE_IMAGE_REFERENCE"
```

The image reference must be an immutable
`ghcr.io/chronoaiproject/cma-api@sha256:...` reference (or `cmaeg`, `image-manager`,
`runtime`, `web`). Record the registry root digest, not a Docker image config ID.
The source is the full 40-character Git SHA. Registry/source/build provenance
must be independently verified by the release artifact workflow. Supplied hashes
and assertions alone are not proof that a process executed that image.

The plan has exactly these fields:

| Field | Contract |
| --- | --- |
| `schema` | `cma-migration-matrix-plan-v1` |
| `candidate` | `{source, image}` with the exact source SHA and digest reference |
| `readers` | Unique `{id, role, artifact}` rows; artifact uses `{source, image}`; roles include `historical`, `current`, `candidate`, `recovery`; exactly one candidate |
| `fixtures` | Unique `{id, sha256, writer, policy}` rows; writer uses `{source, image}`; policies include `retained`, `disabled`, `enabled`; both candidate writer policies must appear |
| `cells` | Unique `{id, reader, fixture, expectation}` rows for every reader × fixture pair; expectation is `accept` or `refuse` |

Candidate and approved recovery readers must accept every declared fixture.
Historical/current refusal cells explicitly exclude those artifacts for the
affected shape. An approved compatible forward repair may use the same digest
as the candidate. The release reviewer remains responsible for the completeness
of the fixture inventory: removing an entire format from a plan requires review.
The validator cannot discover an omitted store, writer or field.

The evidence envelope has exactly `schema` (`cma-migration-matrix-evidence-v1`),
`plan_sha256` (hash of the exact plan file bytes), `candidate`, `environment`
(`isolated-fixture` or `production`), `configuration_sha256`, `started_at`,
`finished_at` and `cells`. Configuration hash refers to a retained sanitized
configuration record; the release bundle must retain it for review. Timestamps
are canonical UTC ISO strings including milliseconds, and each cell must fit
within the campaign window. An environment label never authorizes production use.

Each evidence cell contains exactly:

- `id`, `reader`, `writer`, `fixture_sha256`, `policy`: match the reviewed cell,
  reader and fixture identities. Reader/writer are complete `{source, image}` pairs.
- `execution`: `artifact`; decoder models cannot satisfy this receipt contract.
- `outcome`: the plan's `accept` or `refuse`, `attempt`: 1, `samples`: a positive
  safe integer, and the cell's `started_at` / `finished_at`.
- `checks`: all required observations must be `true`. Acceptance requires
  `decode`, `replay`, `restart`, `takeover`, `identity_preserved`, `writer_policy`.
  Refusal requires `refused_before_traffic`, `refused_before_write`,
  `identity_preserved`. Name the exercised scenarios in the reviewed campaign
  runbook; a producer must not fill unexecuted checks with `true`.
- `raw`: `{path, sha256}` of a retained JSON receipt containing the complete
  cell observation except `raw` itself. Relative paths stay under the evidence
  directory; traversal, symlinks, empty or oversized files are refused.

Missing/duplicate cells, changed artifacts/fixtures, unexpected fields, incomplete
checks, invalid windows, hash drift, cancellation and attempts above one fail
validation. Keep failed and retried campaigns for diagnosis. A later corrected
candidate needs a separately reviewed campaign and cannot erase earlier failures.
Output contains only aggregate counts or a sanitized failure, without raw input.

## Existing regression coverage and remaining qualification

These tests remain in the normal Backend/Mongo gates. They provide useful
regression coverage and must not be relabelled as historical-image execution:

| Surface | Existing coverage |
| --- | --- |
| Ordinary tool-output writers | `layer2-core/src/sandbox/store/memory/tool_writer_admission_tests.rs`: selected/disabled writers, source receipts, stale generations and retained exact replay; predecessor decoder is a model |
| Feedback writers | `cma/src/store/message_feedback_writer_tests.rs`: real Mongo writes, retained records, disabled cohort, predecessor strict decoder and accepted retries after closure; predecessor is a model |
| Startup guard parity | `cma/src/store/schema_mongo_integration_tests/session_versions.rs`: actual writer versions, restart, exact legacy rows, unknown/malformed versions and routing upserts |
| Runtime downgrade refusal | `.github/scripts/runtime-ledger-downgrade.mjs`: actual pinned predecessor executable against an isolated newer home; successful refusal excludes that reader |

Paths above are under `api/crates/` unless prefixed with `.github/`.
Remaining #1327 scope includes producers exercising actual old/current/candidate
and approved recovery artifacts across all required formats, retained fixture
bytes and provenance verification, restart/takeover observations, and release
qualification bound to the production candidate. No approved recovery plan or
production pass is checked in by this slice. #1554 remains unverified until its
complete matrix, startup and recovery evidence is delivered.
