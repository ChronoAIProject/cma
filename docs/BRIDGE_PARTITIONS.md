# Bridge partition status

Bridge managers can inspect retained partition generations without sending work
or changing the retained Agent conversation. Both endpoints require current
Bridge management; organization access is rechecked through the end of the read.
Each existing target resource also requires its own current read permission.

| Operation | Result |
|---|---|
| `GET /api/v1/bridges/{id}/partitions?limit=25` | Allocation-ID ordered summaries with an opaque Bridge-scoped continuation. |
| `GET /api/v1/bridges/{id}/partitions/{allocation}` | One exact generation, up to 64 pending Channel commands and its latest correlated Channel response. |

The list defaults to 25 rows and accepts limits from 1 through 25. Preserve
`next_cursor` unchanged; it identifies only this Bridge and operation. It grants
no authority. Responses use `Cache-Control: private, no-store`.

Each summary includes the allocation `id`, `generation`, allocation `revision`,
pinned `config_revision`, `current` head flag, allocation `state`, placement and
`context_sharing`. Target IDs identify the exact Sandbox, Workspace, Worktree and
Agent. `target.status=available` lists existing resources in ancestry order;
`reserved` identifies a target whose resources are not present yet. If any
existing target lacks access, target, source and activity are all `redacted`.
Read or authority failures return an error rather than an empty success.

`source.status=available` reports the first accepted message's actual platform
chat and sender identity, optional display name and normalized conversation kind.
These describe the first source, not all participants of an intentionally shared
partition. Render labels as text. Display names have controls removed and are
bounded to 256 characters. If that exact accepted source is no longer available,
`source.status=unavailable` is explicit; hashes and later messages cannot supply
replacement labels. Message content, credentials and internal authority records
are never included.

Available activity contains persisted `agent_status`, ascending `pending`
commands and `latest`. Every command correlates the exact allocation, Agent,
accepted Channel source, command ID and response ID. UI/API-origin work is
excluded. `waiting_for` distinguishes `worker`, `approval`, other `action` and
`none`; delivery is the retained Agent runtime observation. This does not prove
current local worker connectivity or external platform reply delivery.

`pending_truncated` means more than 64 pending candidates exist.
`source_unavailable` means at least one bounded candidate lacks its retained
accepted source; those candidates are omitted. A null latest response can
therefore accompany incomplete source evidence. An absent target Agent has
`activity.status=unavailable`. Snapshot timestamps describe observations and
can differ between the partition summary and command activity.

Historical reads use retained Channel source archives after callback-inbox expiry.
They never reconstruct an archive, invoke a command, retry delivery, reset,
detach or remove resources. Existing allocation cleanup routes remain separate.
