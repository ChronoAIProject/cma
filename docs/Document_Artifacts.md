# Persistent document artifacts

CMA documents are user-owned Markdown artifacts, independent of the Sandbox or
Agent that produced them. The portal exposes a registry at `/artifacts`, a
rendered editor at `/artifacts/{document_id}`, and public REST operations under
`/api/v1/documents`. The same operations are available to external callers and
both repository SDKs. The public OpenAPI documents describe their schemas.

## Persistence and ownership

Agent Manager stores metadata, revision references, sharing decisions, comments
and idempotency receipts in its own `cma_document_artifacts` MongoDB collection.
Metadata commits use majority reads/writes and compare-and-swap revisions.
Each source version is an immutable Markdown object in the existing S3-backed
storage service. Its server-generated key is:

```
documents/{sha256(owner)}/{document_id}/{sha256(markdown)}.md
```

The owner is derived from verified caller identity. Browsers cannot select object
paths, storage URLs or ownership. A runtime capability is bound to its exact
Agent, Sandbox boot and native conversation; the core revalidates that identity
after the object write and before committing metadata. Runtime publication may
update only documents actually authored through that Agent's capability. A
REST-supplied origin label grants no runtime authority.

Documents remain after their originating Sandbox or Agent is removed. Deleting
a Sandbox does not delete this independent registry. This release provides no
document deletion API; document sources and revision references are retained.

## Storage deployment contract

`CHRONO_DOCUMENT_BUCKET`, `CHRONO_STORAGE_CLIENT_ID` and the Secret reference
`CHRONO_STORAGE_CLIENT_SECRET` enable storage. `CHRONO_PLATFORM_BASE_URL` supplies
the canonical HTTPS origin. The existing machine identity requests an OAuth
token using form-encoded client credentials and the scopes
`proxy object:create object:read object:download`.

The storage proxy is
`/api/v1/proxy/s/chrono-storage-service/api/buckets/{bucket}/objects`.
An upload includes generation 1 and the source digest; CMA checks the returned
key, size, digest, generation and fence version before acknowledging a save.
Reads use the service's bounded download endpoint. Redirects are disabled and
credentials never leave the configured origin. Tokens are refreshed before
expiry, with one retry after a rejected token. Credentials and source are not
logged. No new infrastructure, NyxID deployment or runtime storage credential
is required.

Each object read or write has a 30-second total deadline, including waiting for
the token-refresh mutex, token acquisition, the single 401 refresh/replay and
response-body validation. Individual HTTP requests remain bounded to 12 seconds
with a 3-second connection limit. Expiry returns typed `DocumentError::Unavailable`
and drops the local operation; it cannot undo a write already accepted by the
storage service. Content addressing and generation fencing remain mandatory,
and no background retry is spawned. See [the dependency contract ledger](Dependency_Contracts.md)
for deterministic coverage and the remaining provider acceptance evidence.

Without a configured bucket, storage-dependent requests return an explicit
unavailable response and the document tools are omitted from discovery. There
is no memory-backed production fallback. Failed metadata commits can leave an
unreferenced immutable blob; they cannot overwrite a live source or acknowledge
a document that has not been durably recorded.

## Editing and concurrency

`revision` orders all metadata mutations; `source_revision` identifies the
latest source/title change. The editor fetches the exact source revision, keeps
unsaved drafts across portal navigation, and preserves their original revision
baseline. Comments or sharing changes cannot silently authorize overwriting a
newer source. A conflict offers authoritative reload while retaining the draft.
An uncertain save reuses the exact request body and idempotency key, including
when the user navigates away and returns.

Markdown supports headings, quotes, lists/checklists, tables, code fences,
images and Mermaid diagrams, with editor extensions for dates, fonts and
collapsible content. Diagrams render locally with strict Mermaid settings and
sanitized SVG embedded as an isolated image, so diagram styles cannot affect the
portal. No remote diagram service receives document text. Embedded
raster images are bounded, and the full source must fit the UTF-8 document limit.

Limits are 1 MiB source, 240-byte title, 160-byte search, 8 KiB comment, 256
source revisions, 128 direct shares, 32 share links and 256 comments per
document. Metadata receipts and aggregate size are separately bounded. These
limits are enforced by the service, not only the editor.

## Sharing and comments

New documents are private. Owners may grant an identified user read or comment
access, or create an authenticated share link with one of those permissions.
Only the owner edits source or title. Comment authors and the owner may remove
comments. Revocation takes effect on the next authorized request.

A share link contains a 256-bit random token in its URL fragment. The browser
submits it only through `X-CMA-Document-Share-Token`; it is never placed in a
query string, OAuth redirect or API path. The service stores its hash. Link
access does not make the document discoverable in registry searches. Direct
sharing uses a NyxID subject identifier; link sharing does not require the owner
to know the recipient's identifier.

## Agent publication

The limited CMA MCP endpoint exposes `publish_document` and `update_document`
when storage is enabled. The runtime supplies a stable `request_key`, Markdown
and title; updates also carry the document ID and expected metadata revision.
The service derives the owner and returns a confirmed `/artifacts/{id}` link.
Tools cannot alter sharing or grant access. A failed call must not be presented
as a published document; transport retries reuse the exact original key/body.

Managed guidance V3 adds document publication to the unchanged GitHub V1 and
memory V1 guidance. Newly prepared homes use preparation revision 4. Historical
home/profile bytes remain immutable. Retained Agents require an acknowledged
idle reload of their original native conversation before new instructions or
tools can be reported as applied; changing a file alone is insufficient.

When document storage is configured, retained live Agents receive one automatic
managed-context revision at a positively observed idle boundary. Active turns,
approvals, first messages, pending lifecycle work,
legacy queued messages and the durable command queue defer admission. The
operation is committed with the parent and dispatch fences before any runtime
write. Its exact desired guidance and recognized previous prefixes are retained
for recovery; the immutable creation profile and receipts are not rewritten.
Native autonomous goals also defer refresh: the old bridge must explicitly
report no goal, and any goal observed on that connection keeps new refresh
admission disabled until a later connection proves it safe. An already-attached runtime
without that observation remains deferred rather than being interrupted to
discover its state.
The adapter supplies a readiness receipt bound to that exact conversation,
runtime generation, boot, home and operation sequence before admission. The
transaction rechecks idle state and both queues before persisting that receipt;
a racing owner message wins. Recovery can use only this recorded receipt when
the goal snapshot is unavailable on a new connection. A changed runtime identity
retires the obsolete maintenance fence without advancing the applied revision,
then leaves the conversation usable for a fresh safe attempt.

The adapter uses the existing supervisor filesystem API to read and replace
only that Agent's `AGENTS.md`, with its current boot identity and expected file
digest. It replaces the longest recognized managed prefix and preserves the
remaining owner instructions byte for byte. Then it closes the idle native
subscription and loads the same conversation ID with the existing working
directory and current MCP bindings. Model, reasoning effort and permission mode
are restored before success is acknowledged. The sandbox process, persistent
home and dirty workspace remain in place. This path works with the retained
pinned bridge; it does not require the new owner-instructions extension.

Only the acknowledged operation advances `managed_guidance_revision`.
Uncertain file writes, reloads or lost acknowledgments retain the dispatch fence
and replay before the conversation is reattached. Repeated file writes do not
duplicate the managed prefix. A refusal before any effect leaves user input
available and delays the next maintenance attempt. An existing mutable prompt overlay receives that same managed prefix atomically
with the acknowledgment, preserving its owner text and preference revision.
After this revision is applied, owner prompt edits continue preserving the
managed V3 prefix.

The pinned native regression executes both document tools after refresh and
checks the next inference uses the original thread ID, retained history,
selected model/effort and unchanged dirty file. Domain tests additionally cover
busy-state refusal, both queue types, recoverable acknowledgment and one-time
completion. These are local protocol/domain proofs; production verification
uses an isolated test Agent after deployment.

## Release and verification

After a durable `RefreshContext` intent or mutable Agent preference overlay has
been admitted, an API rollback must retain compatible readers and recovery for
those records. An older binary that rejects them is not a safe rollback target.
Web rollback can retain the new compatible API and configured runtime default;
it does not authorize replacement of users' live runtimes.

Promote CI-built API, Web and compatible runtime artifacts. Apply only CMA's
configuration and deployments. Preserve existing runtime identities, PVCs,
native conversations and admitted work; changing the default runtime must not
force replacement of an active user Sandbox. Keep prior Web assets available
for already-open tabs during rollout.

Acceptance includes authenticated and anonymous routing, toolbar operations,
Files split/draft retention, document editing/reopening, source conflict
handling, sharing/comment/revocation, exact S3 persistence, confirmed Agent
publication, and retained-conversation continuity. Use isolated test Agents and
documents. Do not mutate user conversations or owner-adopted fixtures merely
to demonstrate the new feature. Record the actual deployed digests and measured
results separately; source completion or passing local tests alone is not
production acceptance.
