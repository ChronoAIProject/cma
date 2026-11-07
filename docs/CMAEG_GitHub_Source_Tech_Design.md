# GitHub source implementation map

The 2026-11-04 source proposal is superseded by
[Native GitHub integration and ordinary channel execution](CMAEG_GitHub_Integration_Design.md).
In particular, its static attestations, one-hour proof expiry, automatic App API
reconciliation, execution groups, batching, runtime publication service and
structured Agent-result interpretation are withdrawn. None is an onboarding
prerequisite or fallback. The owner's current release decisions govern.

| Boundary | Implementation and responsibility |
| --- | --- |
| App control plane | `github/integrations.rs`, `mutations.rs`, `http.rs`: owner-scoped registration, credential custody, independent App verification and terminal retirement |
| Installation authority | `github/bindings.rs`, `model.rs`: pending requests, admin approval/re-approval, exact approved repository sets, CAS and replay |
| Lifecycle | `github/installations.rs`, `receiver.rs`: immediate tightening, durable exclusions, no positive-event widening, compact ignored outcomes |
| Storage | `github/store.rs`, `binding_store.rs`, `schema.rs`, `startup.rs`: sealed records, transactional guards, version-6 feature-gated initialization and bounded receipt TTL |
| Channel selection | Saved `Source::Github` identities/allowlists; dynamic owner/binding checks and current source diagnostics |
| Ordinary execution | `github/receiver.rs` plus existing routed admission: receipt-first recovery, compact System Event, explicit non-human consent, task ID equals admission ID |
| Serialization | CMA's existing per-Agent durable command lane, capacity 64; both bindings explicitly opt into the deployed bounded queue-lifetime policy |
| Board publication | chrono-goals scripts and sync skill using managed `gh api`; check/build, exact-head CI and merge recovery |
| Daily trigger | chrono-goals Actions cron/dispatch → authenticated durable Custom channel → the same Agent |

The gateway does not know the board repository, sync commands, publication branch,
PR schema or business outcomes. It does not parse issue text for instructions or
select an Agent from webhook payload fields. The Agent fetches current business
data through separately authorized NyxID GitHub access.

The GitHub capability reports `ownership_verification: true`,
`automatic_reconciliation: false`, `durable_dispatch: true`. Eligible business
webhooks commit a pending receipt before attempting ordinary routed admission.

The former claim that all channel messages have durable unlimited waiting was
incorrect: existing-Agent binding turns currently use the common five-minute
Responses admission deadline. Expiry is a definite never-dispatched failure,
`agent_command_queue_timeout`, rather than uncertainty. The separate queue-policy
change must preserve ordinary chat/UI behavior and be verified online with two
quick events behind a complete sync.

[The protocol](CMAEG_Protocol.md) defines the wire contract.
[Ingress](CMAEG_GitHub_Ingress.md) defines intermediate release behavior.
[Online verification](CMAEG_GitHub_Online_Verification.md) defines the actual rollout.
[Custom durable execution](CMAEG_Custom_Durable_Execution.md) and
[GitHub Runtime transport](CMA_GitHub_Runtime.md) retain their existing contracts.
Historical test counts and online observations in the old proposal do not prove
this release, deployed flags, Agent permissions or the completed board loop.
