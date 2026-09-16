# Attachment retrieval boundary

Attachment input remains unavailable. CMA refuses the entire attachment-bearing
message; it does not discard media and execute the caption. No retrieval worker,
connector grant issuer, persistent custody, attachment read grant or Agent media
consumer is composed by this foundation. The capability catalog is unchanged.

## Retained connector evidence

NyxID commit `0ac096837c7436b05b71b374207a42320776715d` forwards references without
downloading their bodies. Its Telegram adapter places an opaque `file_id` in the
attachment `url` field. This requires a separate provider-authorized resource
lookup; an opaque handle is not an HTTPS destination. Lark and Feishu construct
message-resource URLs on `open.larksuite.com` and `open.feishu.cn`, with original
message and file/image handles. The adapter explicitly requires a bot tenant
token to fetch that resource. The retained channel relay handlers and service
catalogs expose no scoped attachment fetch-grant operation.

A selected NyxID key is not a provider bot/tenant token and must never be sent to
those resource hosts. A signed callback reference does not authorize fetching
its URL. No provider host or credential has been enabled by this change.

## Private adapter contract

The private CMA adapter requires a trusted connector authority to issue a grant
with an independently approved exact HTTPS host and resource URL. The grant
binds the owner, Bridge and revision, bot, platform, source and complete inert
attachment reference. It has one nonrenewing deadline. Its authorized URL may
differ from an opaque provider handle; the callback cannot issue this grant.
The only production authority currently returns `Authorization` unavailable.

The prepared reqwest transport resolves once and pins a fresh connection pool to
the vetted addresses, retaining the original TLS hostname and certificate checks.
It rejects mixed safe/unsafe DNS answers, private and special ranges, mapped IPv4,
alternate ports, IP-literal hosts, userinfo and fragments. It disables inherited
proxies, redirects and automatic decompression. It accepts only HTTP 200 with no
content encoding. The interface accepts no arbitrary headers, cookies or NyxID
keys. A response peer must match an approved address and port.

Each attempt has a hard 15-second deadline including authorization, resolution,
connection, streaming, scanning and final authority revalidation. Grants can
shorten that deadline. The transport retains the attempt's capped deadline,
which is checked again after DNS immediately before initiating a connection.
Streaming is limited to at most 1 MiB, even with absent or
incorrect `Content-Length`; truncation, excess bytes and stream failures discard
the buffered result. Each transport chunk is checked against the remaining
budget before copying, and the accumulator reserves only the grant's limit.
This is transient private memory, with no publication or
durable custody operation.

PNG, JPEG and PDF are the only MIME candidates for a hypothetical grant. The
response must have exactly the authorized MIME and a matching byte signature.
Signature corroboration does not establish format validity or malware safety.
A separate trusted scanner must approve the complete bounded bytes, then the
original connector authority must revalidate the same grant before bytes return.
Missing, unavailable, rejected or timed-out scan/authority results release no
content. The only production scanner currently reports unavailable.

## Evidence and enablement limits

Injected authority, DNS, transport and scanner fixtures cover scope substitution,
mixed/private/mapped DNS, rebinding, redirect/status refusal, streaming bounds,
truncation, MIME mismatch, scanner refusal, revocation and elapsed deadlines.
Local TCP fixtures exercise the real HTTPS transport's address override, original
TLS name, invalid-TLS refusal and inherited-proxy suppression. They do not claim
successful provider downloads, valid media decoding or a working malware scanner.

Production retrieval requires a real connector grant and credential contract,
applicable scanning, expiring owner/source custody and narrowly scoped read
grants, plus an Agent implementation that supports the resulting attachment
reference. The existing approved Agent artifact publication registry has a
different sharing authority and is not used to expose inbound media. These
remaining requirements and live provider evidence keep issue #867 open.
