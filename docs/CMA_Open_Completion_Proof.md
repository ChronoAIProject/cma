# Native ACP open-completion proof

This prototype adds a read-only `cma/open-completion` extension to the pinned
Codex ACP bridge. A positive receipt means that this bridge observed a complete,
successful ACP `session/new`, `session/load`, or `session/resume` setup for the
exact original working directory and MCP descriptors. The receipt is equivalent
to that successful setup, including the outer load's history streaming. It does
not establish installed tool names, schemas, readiness, credential validity,
provider authority, or successful persistence of an API confirmation.

The implementation is confined to Layer 3's native-provider patch and runtime
image build inputs. There is no API caller, recovery selector, image promotion,
or runtime replacement in this increment. It remains separate from bounded
confirmation retries after a successful handshake.

## Wire contract

`{"action":"support"}` returns `{"supported":true,"version":1}`. A read takes:

```json
{
  "action": "read",
  "sessionId": "native-conversation-id",
  "bindingDigest": "v1:<64 lowercase SHA-256 hex digits>",
  "challenge": "caller_random_challenge_1234"
}
```

The challenge contains 16–128 ASCII letters, digits, underscores or hyphens;
the session ID is nonempty and at most 256 characters. A successful read returns
`complete`, `version`, the supplied `sessionId` and `challenge`, the exact
`bindingDigest`, a process-instance UUID `processNonce`, an open UUID
`openGeneration`, and the ACP integer `sessionGeneration`. The open UUID changes
after each successfully observed setup, including exact cached reuse. A refusal
returns only `complete:false`, `version`, `sessionId`, and `challenge`; it contains
no native diagnostics or configuration. Malformed requests receive invalid
params. Support alone provides no completion evidence.

The digest input is UTF-8 canonical JSON for exactly
`{"cwd":<original cwd>,"mcpServers":<original descriptors>}`. Object keys sort by
JavaScript string order, object values use JSON encoding, and arrays preserve
order. No whitespace is inserted. Absent `mcpServers` becomes `[]`. Prefix the
SHA-256 lowercase hex result with `v1:`. Cwd is the original absolute string,
without symlink resolution or path normalization, and must exactly equal the
native returned cwd and current ACP state cwd. A caller must retain this original
snapshot; descriptors newly assembled during recovery cannot substitute for it.

Snapshots are cloned before the first await, deeply frozen and capped at 64 KiB
with at most 32 descriptors. Names must be unique ASCII letters, digits,
underscores or hyphens, 1–64 characters, so sanitization cannot collapse distinct
names. At least one HTTP descriptor must be a normal CMA tool family:
`cma_access`, `cmaeg`, or `cma_channel`. Each qualifies independently; provider-only
or channel-only Agents do not need an access binding. Other descriptors, when
present alongside a recognized family, remain part of the same exact snapshot and
native-config checks. Unsupported snapshots permit ordinary ACP behavior but
cannot obtain a completion receipt.

The current hosted `acp_server` constructors produce:

| Family | Descriptor | Headers | Native startup policy |
| --- | --- | --- | --- |
| Access | `cma_access`, HTTP, `/api/v1/agent-tools` | One `Authorization` header | Required when present |
| Provider | `cmaeg`, HTTP, `http://127.0.0.1:9093/` | Empty array | Optional |
| Channel | `cma_channel`, HTTP, `/api/v1/channel-tools` | One `Authorization` header | Optional |

The handshake appends these in channel, provider, access order, omitting absent
families. The digest retains that array order and the actual endpoint and headers;
this extension does not infer authority from a family name or endpoint shape.

Reads inspect only local retained state and the native child's exit/signal
status. They perform no native discovery, MCP or model calls, file writes,
close/load/resume, turn creation, or recovery action. Raw descriptors, headers,
URLs and native errors never enter the receipt.

## Canonical vectors for later Rust integration

`api/crates/layer3-adapter/src/codex/open_completion_vectors.json` contains eleven
fixed test vectors with `input`, exact `canonical` UTF-8 text, and `bindingDigest`.
They cover each normal constructor shape, combined handshake order, object-key
reordering, array reordering, Unicode plus JSON escaping, absent/empty servers,
absent headers, and null metadata. Canonical-only cases that are outside the current typed constructors
are marked `exerciseReceipt:false`; these do not claim native acceptance. The
Node tests feed the fixed expected digests directly to the real receipt registry
for supported shapes, rather than only comparing a separate helper. An independent
Python implementation also verified the UTF-16 ordering and string/array encodings when authoring the vectors.

For Rust, serialize the original descriptor values, retain arrays exactly, and
sort object keys by UTF-16 code units to match JavaScript. Default Rust string or
UTF-8 ordering differs for supplementary characters: the emoji key in the Unicode
vector sorts before `U+E000`. Emit Unicode as UTF-8 without ASCII escaping and use
JSON escaping for quotes, reverse solidus and control characters. Do not reorder
headers, normalize URLs or cwd, omit an empty `headers` array, or add absent
metadata. The current normal constructors contain only objects, arrays and
strings; an extension introducing numeric metadata must also match ECMAScript
number serialization rather than assuming Rust's default formatting is identical.
A later API caller must restrict its supported descriptor subset to the current
typed constructors until broader metadata has separate interoperability evidence.
These fixtures provide an interoperability contract without adding a Rust caller
or claiming Rust execution in this increment.

## Evidence and invalidation

An outer setup is serialized with other opens and instruction refreshes. At most
32 operations may wait or run. Targeted operations capture the original state,
generation and close-fence validity before entering the queue. If a close,
replacement or instruction refresh intervenes, queued load/resume and instruction
mutation fail with invalid request before invoking their original callback. They
cannot reopen a closed target or write files after that close. A new operation has
no target ID at enqueue; its existing native-ID close tracking applies once that
identity becomes known. Before replacement starts, its previous receipt is retired.
The exact native start/resume request must contain the transformed original descriptors, and a bounded config read must show no requested name in
user/project MCP configuration. This conservatively refuses cases where ACP's
conflict filtering or native configuration merging obscures the original setup.

A fresh `session/new` receives a new native identity. An existing conversation
has two eligible paths:

- A current exact prior receipt can carry the original frozen configuration
  evidence through a cached load/resume. The request digest must match, and the
  old state reference and generation must still be current. A different request
  retires the receipt and cannot relabel the native session's retained settings.
- A cold load/resume requires a complete, empty native `thread/loaded/list`
  immediately before the native resume. It is one request with limit 128, no next
  cursor, and a three-second evidence deadline. Requiring empty inventory is
  deliberately stronger than target absence: it excludes unobserved loaded
  subagents that could independently resume the target. Each config evidence read
  has the same deadline. Errors or incomplete inventory suppress proof while
  leaving ordinary ACP setup behavior intact.

The receipt is installed only after the entire outer operation returns
successfully. It binds the actual installed `sessionState` object, ACP generation,
original frozen snapshot, open UUID, and the one native process lifetime. State
installation, close fences, lower native unsubscribe, and native `thread/closed`
retire previous proof before asynchronous replacement/close work. A close or
prompt racing an open invalidates its candidate. Native fork, review and turn
starts are also tracked from before dispatch through their response, excluding
background operations already in flight when the inventory is read. A failed setup, including failed
history streaming after native resume, produces no proof. Process exit or signal
clears all receipts; the read also checks child liveness synchronously. There is
no respawn path in this pinned bridge. The registry retains at most 32 entries,
evicting oldest proof conservatively at capacity.

Successful ACP setup can precede optional MCP startup failure. The native test
intentionally supplies an unavailable optional `cmaeg` endpoint and still obtains
a completion receipt. The existing patch keeps `cma_access` required when present and `cmaeg`
and `cma_channel` optional. A provider-only `cmaeg` setup can therefore complete
without an access binding and without proving provider tool availability. Callers must not interpret this extension as a tool catalog receipt or
use it to bypass their current binding, reservation, generation, credential,
request, lineage, or runtime-boot authority checks.

## Retained-runtime compatibility and recovery boundary

The currently affected 46c runtime composition does not implement this extension.
It cannot retroactively prove an unobserved completion. `method not found`, absent
support, `complete:false`, a changed process nonce, or a mismatched snapshot are
explicit unavailable evidence. There is no generic fallback to `session/list`,
local ACP state, an output checkpoint, persisted conversation identity, or a new
`session/load` with newly supplied descriptors. Those observations do not show
which setup completed or which configuration the native process retained.

The pinned native 0.147.0 source can silently rejoin an already loaded thread and
ignore resume configuration while reporting success. The real native regression
reproduces that behavior with changed MCP descriptors. Once prior proof is lost,
even resubmitting the old descriptor snapshot to that loaded native thread cannot
reconstruct it. An incomplete original open likewise cannot acquire a receipt
through this read-only method.

A future API integration may use a matching retained receipt to finish the
original confirmation transaction after rechecking all current authority. It
must preserve the original exact binding, validate a fresh challenge response and
the current runtime/process/generation fences, and fail closed on missing proof.
The provider descriptor is a constant loopback endpoint with no authorization
header, so its digest does not encode the proxy's upstream URL or credential
custody generation. The caller must separately validate the current provider
binding and exact runtime/proxy observation. This increment does not implement or
authorize that path.

Affected old processes need a separately reviewed and authorized lifecycle
migration/recovery: preserve the Agent home, worktree, persisted conversation and
user files; validate the replacement runtime and original request authority;
retire the old process through existing lifecycle fencing; and perform an
observed cold setup of the same conversation in the new native process before
confirmation. A supported image, checkpoint progress, or new public generation
alone cannot authorize the transition. No reset, conversation deletion, synthetic
success, automatic recovery, or deployment is part of this prototype.

## Pinned validation

The patch accepts only predecessor ACP SHA-256
`84f9670793e2848b6c49e0541a325223239eeabd0e406d9a586dbcd2935cb85c`, derived
from ACP 1.4.0 upstream SHA-256
`ba74f54f7c89ba61858e544eb3afefbb0c1b90f2b23a136664828816a8430dbf`.
The final image composition is pinned in `infra/runtime/Dockerfile`; its build
applies this patch after turn steering and runs both new test files. Compatibility
tests retain exact pre-patch reconstruction for the legacy instruction-recovery
fixture, and pin the final compositions with the access/model patches removed.

Local validation uses the real `@openai/codex@0.147.0-darwin-arm64` binary from
tarball SHA-256
`493de2d788c6ff01de222307c767231c1e6c802d5dd808523550fad6d972f4d3`.
All network fixtures listen on loopback and use synthetic values. Focused tests
cover immutable snapshots, read purity, exact carry, malformed/paginated inventory,
unknown loaded state, descriptor/config ambiguity, setup/history failure,
serialization, close/prompt races, state/generation replacement and process death.
Real native tests exercise access-only, provider-only and channel-only new, cached
load/resume and cold load/resume after process restart; ignored changed access and
provider descriptors; provider-only optional-server failure; and native SIGKILL.
Receipt reads preserve rollout bytes, dirty files, MCP request counts and model
request counts. Docker builds, Cargo compilation, production writes and lifecycle
actions are outside this local validation.
