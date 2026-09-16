# Attachment retrieval boundary

CMA composes one opt-in attachment path: a single original Discord `text/plain`
`.txt` attachment of 1–16,384 bytes, converted into bounded UTF-8 user text for
the existing Agent command pipeline. `CMA_DISCORD_TEXT_ATTACHMENTS=true` enables
new admissions; the default is false. It does not enable images, PDFs, audio,
OCR, arbitrary URLs, generic Agent attachment references, or outbound files.
Unsupported, multiple, normalized-only and out-of-policy files retain the typed
whole-message `attachment_download_required` refusal, including private Telegram
files. Captions are never executed on their own when a file is refused.

## Source and authorization

The pinned NyxID implementation is `0ac096837c7436b05b71b374207a42320776715d`.
Its selected Discord adapter verifies the platform webhook signature, retains
raw platform data, and the relay signs the exact callback body SHA-256. The
normalized Discord attachment array is empty in that revision. CMA therefore
reads only the exact original `MESSAGE_CREATE` attachment object in authenticated
`raw_platform_data`; user text, generic callback URLs, proxy URLs and normalized
attachment references cannot issue a fetch grant. Existing explicit human,
original-event, sender/chat correlation, group, loop and selected-binding gates
still apply. This is source/fixture evidence; it does not establish a deployed
Discord Gateway delivery path or a successful live provider download.

Discord's official attachment and signed-CDN contracts are pinned at
`discord/discord-api-docs` commit `52ecc8a1908cc3a8dadb55671aef696bb1ed270e`,
`developers/resources/message.mdx` and `developers/reference.mdx` (Signed
Attachment CDN URLs). CMA requires `https://cdn.discordapp.com/attachments/`
with exact source channel and attachment snowflakes, a safe ASCII `.txt` filename,
and exactly one each of `ex`, `is`, and `hm`. It bounds the documented signed
expiry and rejects ambiguous query fields, encoded paths, wrong IDs, alternate
hosts, credentials, fragments and ports. Discord verifies its own MAC; CMA does
not manufacture or claim independent validation of that MAC.

The deferred worker obtains current authority for the original owner, Bridge
revision, selected bot/key, exact inbox and target. Opening private URL custody
uses the existing live invoke transaction fence, including current credential,
grant and resource checks. Each download authorization/revalidation refreshes
the same exact authority without extending the original attempt. No NyxID key,
provider bearer, cookie or arbitrary callback header is sent to the CDN.

## Capture and Agent representation

Callback acceptance remains within its five-second path and performs no CDN IO.
It atomically stores original receipt/source metadata and the encrypted URL in
private `bridge_text_attachments` custody. URL custody expires at the earliest
of the CDN expiry, callback retention, or fifteen minutes from admission. Reads
check logical expiry before decryption even when Mongo TTL cleanup lags. There
is no public URL/object endpoint or automatically granted sharing permission.

Retrieval runs in the existing bounded Channel inbox worker. It resolves once,
rejects unsafe or mixed DNS answers, pins a fresh connection pool to approved
public addresses, retains TLS hostname/certificate checks, and checks the actual
peer. Proxies, redirects and decompression are disabled. Only HTTP 200 with no
content encoding and exact `text/plain` is accepted. Streaming is limited by the
signed original byte count, including absent/incorrect Content-Length, with a
single nonrenewing fifteen-second deadline covering authority and network IO.
The complete result must match the original size and be valid UTF-8 with no
control characters except tab, CR and LF.

Text uses CMA's ordinary untrusted user-input policy; there is no malware-clean
claim. The first committed text and SHA-256 become immutable under the same
current invoke transaction fence. Concurrent or uncertain retries recover that
capture and cannot change the command's content. The original callback ID,
attestation, caption and deterministic command/response identity are preserved.
The Agent receives canonical JSON text containing the caption, file name, MIME,
size, digest and `untrusted_text`. It is ordinary user input and cannot be
interpreted as a reserved Channel control. The signed CDN URL is excluded.

New command admission requires unexpired custody. Once admitted, that user text
and its exact source follow the Agent's retained-history policy, so later file
expiry or callback TTL deletion does not erase or change an existing turn.
Normal Agent access policy governs retained text; this path never fabricates
human authorization, ManageSharing authority, or a general artifact read grant.
Retrieval failure and expiry remain explicit durable worker states
`text_attachment_unavailable` and `text_attachment_expired`, with no caption-only
fallback. The original transport receipt remains an acceptance receipt, not a
claim that downloading or Agent execution succeeded.

## Other media and rollback

The existing PNG/JPEG/PDF foundation still requires an independent connector
grant and trusted scanner. Signature checks are not malware scanning, and its
only production scanner remains unavailable. Telegram's opaque `file_id` needs
a provider-authorized lookup; Lark/Feishu resource URLs need a bot tenant token.
Selected NyxID keys cannot substitute for those credentials. Those paths stay
unavailable.

Deploy supporting readers and workers everywhere before enabling the new switch.
The new persisted `discord_text_attachment` event fails closed on old readers;
older per-row workers cannot execute only its caption and continue discovering
unrelated rows. Disable new admissions before rollback. Already admitted text
commands require a supporting reader for their retained source; rolling back to
an older binary suspends those rows rather than rewriting their receipts or
prompts. Keep a supporting worker until accepted attachment work is drained, or
explicitly retain the suspension until forward recovery. The switch does not
reclassify an already retained refusal on exact replay. This boundary requires
live coexistence/rollback acceptance; it is not a claim that those drills passed.

Focused signed-source, network, expiry, custody, immutable-capture and retained
command fixtures exercise the composed text path. Injected CDN transport is
identified as synthetic evidence. Real platform delivery, general media, scanner
policy, and live personal/org provider acceptance keep issue #867 open.
