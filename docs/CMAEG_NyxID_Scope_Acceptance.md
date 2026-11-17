# Private NyxID connection scope acceptance

Tracking: [CMA #1231](https://github.com/ChronoAIProject/cma/issues/1231).
The scope and immutable-profile rules in [the protocol](CMAEG_Protocol.md#updating-a-cma-profile-binding-without-losing-work)
remain authoritative. PR #1232 preserves the Manager's named refusal at the
Gateway HTTP boundary: HTTP 409, `application/problem+json`, code
`provider_rejected`, with a fixed actionable explanation of the pinned profile.
The upstream detail is not returned, so a provider cannot echo private connection
endpoints or credentials through this problem.

An existing profile-managed channel retains its approved `service_scope` on an
unchanged PUT, including personal custom service slugs and connection UUIDs.
Changing the bootstrap key allowlist does not rewrite that admitted profile or
its Sandbox. A new profile scope on an existing binding is refused by the
Manager before changing its binding, credentials, resources or retained work.
The Gateway then leaves its sealed record and version unchanged. Keeping the
original profile permits a later channel settings update. Neither resupplying
the same key nor substituting the connection UUID for its slug bypasses the
immutable-profile rule.

The local management regression
`retained_private_connection_scope_succeeds_before_and_after_refused_expansion`
uses both private slugs and UUIDs in the retained scope. It proves successful
unchanged PUTs before and after six refused expansion attempts, exact retained
sealed bytes/version, safe typed errors, and unchanged provider request identity.
`refused_profile_scope_expansion_preserves_the_channel_and_allows_unchanged_retry`
retains the original reported reproduction. The independent Manager test
`unsupported_updates_and_reconciliation_retries_preserve_original_work` proves
that the named refusal leaves the original Agent, conversation, worktree,
Workspace and Sandbox intact. The Gateway provider is an HTTP fixture; neither
test proves an actual private NyxID connection works.

## Required external evidence

The acceptance owner and NyxID integration owner must run two distinct cases
against the release candidate with a dedicated owner-authorized fixture:

1. **Previously approved scope:** prove the actual private connection proxy and
   a channel event using that connection succeed, before and after an unchanged
   PUT. Record retained scope/version and completion receipt. If a required
   existing consumer contract includes expansion, prove that exact expansion;
   the generic immutable-profile refusal is not acceptance of that contract.
2. **Uncommitted new scope:** verify expansion by slug and by connection UUID,
   with omitted and explicitly repeated credentials, either succeeds with the
   exact approved grant or returns the structured actionable 4xx before binding
   effects. Read back unchanged scope/version after refusal and deliver another
   event using the retained scope. Include invalid/revoked connection, wrong
   owner, cancellation, timeout and malformed upstream responses; no failure
   authorizes use of a new scope or broad allow-all grants.

The product owner must identify which proposed expansions were already promised
to existing consumers. That inventory determines whether refusal blocks GO or
the new expansion can be deferred; local code cannot infer that product history.
Do not satisfy acceptance by expanding all service grants or recreating a live
channel and discarding its work. Each evidence packet must identify source SHA,
candidate image digest, environment, time window, sample count, configuration,
and protected raw-result location. No production test or scope change was made
by this audit.

This audit adds regression coverage and documentation only. It introduces no
durable format, writer, migration, public API or deployment setting; the existing
reader floor, writer gates and recovery artifacts remain applicable under
[CMA data migrations](CMA_Data_Migrations.md).
