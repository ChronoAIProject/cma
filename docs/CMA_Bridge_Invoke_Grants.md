# Explicit retained-Agent invoke grants

A fixed-target Bridge requires an explicit grant to use the retained Agent's
existing conversation and context. `FixedSharedAgent` partitioning selects shared
context routing; `UseRetainedAgentContext` is the separate explicit consent input.

`BridgeInvokeService::issue` accepts the actual current Bridge owner or organization
administrator and requires current `ManageBridge` and `Interact` resource access.
It binds the exact Bridge owner kind and issuer, complete configuration generation,
Sandbox/Workspace/Worktree/Agent ancestry, and creation identity. Organization
ownership comes from the retained policy sidecar, never from a matching subject
string. The audit retains the actual issuing human or API actor. Grant authority
cannot be obtained by constructing an ordinary channel `CallerContext`.

Grants can be issued while a Bridge is disabled. Issuance requires current selected
encrypted custody and the exact retained resource. The grant starts active at
revision 1; explicit revocation is terminal at revision 2. Configuration refresh
does not rewrite, revoke, or reactivate the original grant. Revocation needs current
Bridge management authority but remains available when custody has been revoked or
the target has disappeared. Receipt keys are bound to issuer, Bridge, actual actor,
and original request; replay returns the original receipt without restoring a grant.

The Mongo adapter commits grant CAS, immutable audit, and receipt together with
Bridge, encrypted-custody, and retained-resource fences. Concurrent duplicate
requests reconcile one receipt. An audit write failure rolls back the entire
mutation. A serialized grant or historical receipt is data and cannot construct an
execution proof.

Trusted workers call `BridgeInvokeService::authorize` for one exact effect:

- `BindExistingTarget { allocation_id }` binds a current enabled allocation to the
  exact retained target. It requires the current admission generation.
- `AdmitMessage { inbox_id }` checks authority for an exact retained input.
- `ExecuteCommand { command_id }` checks authority for an exact deferred command.

The latter two effects retain their original generation under `DrainOriginal`.
Explicit `SuspendOriginal`, execution emergency stop, grant revocation, resource
identity change, or custody loss refuse subsequent effects. Proof clones retain
the original deadline; provider, resource, or database IO cannot renew it.

The allocation or command consumer must verify its own exact source/reference and
call `MongoBridgeInvokeStore::fence` in the same Mongo transaction as its effect
admission. The fence rereads the current grant, Bridge, original-generation custody,
and retained resource lineage. The fixed allocation adapter currently consumes this
proof. Common command admission and deferred dispatch integration remain separate
work; having this foundation does not imply channel command dispatch is complete.

`current_candidate(&BridgeManagementLease)` returns a current management readiness
observation, including while disabled. It does not mint execution authority.
Setup enablement separately prepares an opaque `BridgeInvokeEnableProof` for the
actual current manager, full Bridge revision/configuration, active grant, and
retained creation identity. Generic fixed-target enablement without that proof
refuses `bridge_invoke_consent_required`. The Mongo enable transaction rereads and
fences the grant and resource lineage together with selected custody and the
Bridge transition, so revocation or configuration/creation changes after readiness
cannot enable the Bridge. An original enable receipt remains historical after
revocation and never restores disabled state or grant authority.

Composition attaches `BridgeInvokeService` using
`BridgeSetupService::with_invoke_grants`. Opening the Mongo invoke adapter installs
its resource handle in the definition adapter without a reference cycle; missing
composition refuses the fixed-target transaction. The standalone memory definition
adapter also refuses these writes because it has no common grant/resource transaction.

A fixed-target grant grants no Sandbox creation, revival, provider creation, or
tool-approval authority. An accepted inbox that has no fixed allocation when the
configuration changes is currently held by the current-generation allocation
admission requirement; any future historical initial allocation path must consume
its verified original inbox source and explicit drain policy.

Focused core tests cover immutable records, receipt identity, and generation
controls. Disposable Mongo contracts under `store::bridge_invokes::tests` exercise
issuance/revocation races, original receipt replay, retained Agent/log preservation,
resource and custody changes after proof, organization owner/issuer separation,
disabled setup consent, fixed allocation consumption, and nonrenewing deadlines.
Setup-control contracts also exercise grant-free connection validation, disabled
issuance followed by guarded enablement, historical setup receipt replay after
revocation, and exact grant/configuration/Agent-creation races before commit.
The repository Mongo contract runner includes this selector.

## Management HTTP API

These authenticated routes require current owner or organization administrator
management authority and return `Cache-Control: private, no-store`:

| Method and path | Behavior |
| --- | --- |
| `GET /api/v1/bridges/{id}/invoke-grants` | Lists safe grant projections; `cursor` and `limit` paginate within the exact Bridge. |
| `POST /api/v1/bridges/{id}/invoke-grants` | Issues explicit consent using `expected_bridge_revision`, exact `target`, and required `shared_context: "use_retained_agent_context"`. |
| `GET /api/v1/bridge-invoke-grants/{id}` | Returns the retained safe grant projection, including terminal revocation. |
| `POST /api/v1/bridge-invoke-grants/{id}/revoke` | Revokes with `expected_bridge_revision` and `expected_revision`. |
| `GET /api/v1/bridges/{id}/invoke-readiness` | Reports `not_applicable`, `consent_required`, or `consent_present` for the current generation, with an optional safe candidate. |

Both mutation routes require `Idempotency-Key`. Unknown authority fields and omitted
shared-context consent are rejected. Readiness describes only explicit consent;
it does not report that connection validation, operational enablement, allocation,
or command execution has succeeded. The safe grant response includes the exact
retained target creation identity and actual issuing actor, while excluding full
configuration, connector/custody references, resource decision audits, and proof
objects.

`bridge-invoke-grants.json` adds 37 shared public contract cases for these five
operations. Dedicated HTTP tests exercise disabled consent, original response
replay after revocation, safe projections, and rejected authority injection.
