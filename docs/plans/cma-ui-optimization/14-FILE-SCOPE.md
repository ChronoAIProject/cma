# Ticket 14: public computer file boundary

The remaining gap was concrete: public owner file reads could traverse the
supervisor's private Agent-home tree, and every file reader could enumerate other
Agents' upload namespaces. The Agent ID in the URL did not constrain either.
The fix preserves the authorized Sandbox workspace and removes those private
namespaces from the public file surface.

## Acceptance interpretation

The ticket's original file-preservation and Agent-isolation criteria are unchanged.
Their completion is assessed against the repository's existing resource ownership,
using the source contracts and verification below. This interpretation comes from
repository analysis; it does not rely on a new user clarification or scope approval.

Authorized workspace/worktree files belong to the Sandbox/Workspace shared file
surface. Their use or creation by an Agent does not transfer them into that Agent's
private namespace. Preserving that surface's existing listing, editing and download
permissions satisfies file-tree preservation. Excluding the supervisor's Agent-home
tree, foreign Agent upload namespaces and runtime state protects the actual private
file boundaries. The computer panel offers no cross-Agent process listing and
shows only the selected Agent's recorded turn output; permission changes gate that
output and late observations. These distinct resource contracts let both original
criteria be satisfied without treating shared workspace facts as another Agent's
private data or claiming OS process isolation.

## Repository evidence and classification

| Root / data | Source contract | Computer file behavior |
| --- | --- | --- |
| `/home/cma/<workspace>` and `<workspace>/.worktrees/<name>` | `api/crates/layer4-runtime/src/workspace_runtime.rs`, `worktree.rs`; `docs/API_REST.md` Agent home files | Intentionally shared authorized Sandbox source/work product. Preserve listing, hidden files, downloads, versioned edits and typed shared writers. A worktree name or producing Agent does not make these files private. |
| `/home/cma/.cma/agents/<id>/home` | `layer4-runtime/src/config.rs::agent_home`; `layer3-adapter/src/codex/home.rs::AGENT_HOME_ROOT`, `expected_home` and `CODEX_HOME`; `docs/Sandbox_Provider.md` G-A1 | Per-Agent provider instructions, configuration, credentials, sessions and retained state. Public file routes exclude the containing supervisor-control root for owners and shared readers alike. |
| `/home/cma/.cma/provider/codex/auth.json` and other supervisor control data | `layer3-adapter/src/codex/home.rs::SHARED_AUTH_PATH`; `layer4-runtime/src/agent_home.rs` | Excluded by the same public `cma/.cma` rule. Legacy `.cma` at the mounted root is also excluded. Ordinary dotfiles elsewhere remain readable under existing authority. |
| `/home/attachments/<agent_id>/...` | `layer3-adapter/src/agent_attachment.rs::AGENT_ATTACHMENT_DIRECTORY`, `AgentAttachmentRequest::path`; `layer4-runtime/src/agent_attachment.rs::publish` | Keep only the currently authorized Agent's exact namespace. Preserve that Agent's downloads and authorized edits. Omit foreign namespaces and refuse direct foreign paths and deletion of the common attachments root. |
| Runtime journals and checkpoint archives | `layer4-runtime/src/config.rs` (`/var/lib/cma-runtime` default); `layer3-adapter/src/hosted/home.rs` (`/var/lib/cma-transfer`); `layer4-runtime/src/agent_filesystem.rs` | Normal deployment roots are outside `/home`. If configured beneath the mounted home, the runtime excludes them from list/read/write/create/delete, including recursive ancestor deletion, before the owner bypass. |

The repository's pinned Codex layout assigns distinct `CODEX_HOME` directories;
it does not establish OS isolation between processes sharing a Sandbox. This
ticket does not promise to infer provenance of an arbitrary file copied into an
authorized workspace. A symlink or hardlink is not a shortcut around the file
API's existing no-follow/single-link checks.

## Enforcement and compatibility

`AgentFileService` authorizes the requested Agent before checking a public path.
Its public-path guard rejects `.cma`, `cma/.cma`, their descendants and destructive
ancestors. List receipts omit protected entries. Reads and downloads share the
same guarded read method; writes and deletion use the same policy. The uploads
rule compares one complete namespace component to the authorized Agent ID, not
an Agent-ID prefix. Existing finish/revalidation checks still run before data is
returned or a mutation is admitted.

This control-plane guard does not depend on a new runtime wire field or on the
owner flag. It therefore protects these known namespaces even when talking to
an older compatible runtime. Runtime descriptor confinement and configured
state-root exclusion provide the filesystem boundary. The latter needs the
updated runtime when an operator uses a nonstandard state root inside `/home`.

Trusted managed-instruction refresh/recovery is a separate internal operation:
`layer3-adapter/src/hosted/managed_context.rs` deliberately reads/writes the
selected Agent's `AGENTS.md`. Blocking every owner request inside the runtime
would break that operation. The public guard lives in core instead; internal
settings, lifecycle and source APIs remain intact. Public credentials/source
permission checks, file size limits, optimistic write hashes and no-follow
descriptor operations remain in effect.

The environment feature remains `mounted_home`, with its existing capability and
revalidation fields. It means the authorized, filtered Sandbox file surface,
not an Agent-private filesystem. Tickets 15/18 must keep browser/replay private
data behind their own Agent-scoped public contracts, rather than exposing raw
runtime state through the Files tab.

## Verification

- Public REST red/green: an owner could originally download another Agent's
  private home file (HTTP 200); now it is refused, listings omit the control
  root, direct mutations/ancestor deletion are refused, and a shared worktree
  source download plus workspace write/read still succeeds.
- Public REST red/green: `attachments` originally listed two Agent namespaces;
  now owner, typed viewer and typed writer see only the selected Agent. A similar
  ID prefix is excluded too. Own uploads remain downloadable and authorized
  upload/workspace writes remain usable.
- Real runtime HTTP red/green: a configured state file originally returned its
  bytes. Actual filesystem reads now refuse configured state/checkpoint roots
  and symlink/hardlink aliases; listings and mutations are scoped. The same HTTP
  fixture still reads normal workspace bytes and trusted managed instructions.
- All seven ticket06 disclosure integration regressions explicitly expand the
  real collapsed tool controls. Completion order, pending outcome wording,
  terminal replay deduplication, identity, late-history and refresh assertions
  remain; the three complete files pass 48 tests with no exclusions.

Logs reside outside the repository in
`/Users/liuyitian/code/cma-ui-worktrees/ticket14-proof/`: `private-files-red.log`,
`private-files-green.log`, `uploads-red.log`, `uploads-green.log`,
`runtime-file-scope-red.log`, `runtime-file-scope-green.log`, and
`seven-disclosure-regressions.log`. Tests use the private Rust target and actual
nonzero named tests. The final ticket records final merged check counts.
