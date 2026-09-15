# Signed bridge callback inbox foundation

This is an internal implementation foundation for issue #861. Production has no
registered callback endpoint. An adapter can accept work only after resolving the
URL's bridge ID and obtaining a fresh `BridgeValidation` from the selected NyxID
key, route, bot, owner and custody binding. Callback claims cannot create that
proof. Missing custody and unavailable renewable Sandbox authority remain typed
setup failures. Public route composition and deployment are outstanding.

The source contract is NyxID commit
`85607255ee0beadca460a350e0aa6d177edd2fd2`, particularly
`backend/src/crypto/jwt.rs` and `backend/src/services/channel_relay_service.rs`.
The RSA keys under the core ingress `fixtures` directory are disposable public
test data and have no deployed counterpart.

## Verification and authority

The core verifies RS256 with a deployment-pinned issuer and JWKS policy, the fixed
`channel-relay/callback` audience, `relay_callback` token type, selected key,
message and platform. It checks signature expiry, issuance, optional `nbf`, a
maximum 300-second callback lifetime and at most 60 seconds of configured skew.
JWT-provided key URLs, unknown algorithms and missing keys fail closed. The
adapter fetches only the configured HTTPS JWKS endpoint, disables redirects,
limits responses to 32 KiB, caps a key set at 16 RSA keys, expires its cache after
60 seconds and limits refresh attempts to once per five seconds. Failed refreshes
discard cached keys. Old expired keys never gain a renewed lease from a failure.

The exact received bytes are hashed before JSON normalization. The callback's
message/key/platform and signed correlation ID must agree with the headers and
payload. The header timestamp is checked against signed issuance; the payload
timestamp is the independent event timestamp. Both accept NyxID's RFC 3339
fractional seconds and numeric offsets; malformed dates and offsets fail closed.
The selected route is
`conversation.id`; `conversation.platform_id` remains the actual chat identity.
Owner and bot come from the verified bridge binding. Unknown top-level payload
fields cannot select owner, Agent or bot resources. Raw platform data is discarded
during bounded parsing.

An optional relay user token is separately verified for its NyxID audience,
`relay=true`, owner subject and exact selected key. Its inherited scope fields are
parsed and bounded. Both present and absent token outcomes are explicitly
**inbox-only**: this slice neither persists the access token nor converts its scope
into execution, provisioning, renewal, management or owner authority. Workers
will need independent live authority and scope checks before any effect.

Reply tokens are separately verified against the reply audience, type, platform,
selected key, route and inbound message. Accepted reply context is encrypted with
AES-256-GCM, bound to deployment, inbox ID and field name. The reply token is
excluded from the normalized record and receipt, and its retention is capped by
both the signed expiry and the configured retention. Denied senders and disabled
reply policies retain no reply token. Ciphertext has a separate TTL collection;
logical expiry also applies when opening it.

Generic `thread_id` is treated as opaque protected context because a Discord
callback can carry interaction credentials there. It is encrypted beside reply
credentials and excluded from normalized plaintext and partition identity. The
envelope reports only whether protected context was retained. Stable thread
extraction requires the connector-specific capability checks in #862.

## Durable acceptance and limits

The Mongo adapter commits the normalized envelope, exact-body fingerprint,
verified attestation facts, original config generation, owner/bot/key identity,
message and correlation IDs, sender admission decision, encrypted reply context,
and admission rate accounting in one snapshot transaction with majority commit.
A write to the bridge definition fences acceptance against concurrent disable,
emergency ingress stop, retirement and configuration changes.
The same transaction decrypts and checks the exact selected custody reference,
revision and key-state version, including its nonrevoked state, before accepting
new work or returning a duplicate receipt. Revoking custody after signature and
live provider verification cannot authorize an old pending acceptance.

Deduplication uses issuer, bridge, bot, selected key and business `message_id`.
JWT `jti` and configuration revision are not the sole deduplication identity.
A matching accepted body returns its original receipt; different exact bytes for
the same identity produce a replay conflict. Each attempt must still pass current
signature, key-cache and binding verification. Deduplication lasts for the
configured inbox retention, not indefinitely.

The internal adapter bounds concurrent callbacks to 16 and total request lifetime
to five seconds. Core verification caps bodies at 1 MiB, attachments at 32, text
at 512 KiB, tokens at 16 KiB and individual fields at their documented code bounds.
Bridge configuration can lower the body limit. The future HTTP transport must
also enforce the body limit while streaming, before buffering. Durable per-bridge
rate and queue limits are checked inside the transaction; retained accepted rows
count conservatively toward queue capacity until a worker lifecycle is composed.
Signature, exact-body, header and payload checks precede live provider/custody IO.
Their opaque intermediate proof has no persistence authority and retains its
original deadline when bound to a fresh live connection proof.

Only a majority-durable matching receipt can authorize an HTTP 202 response.
Persistence errors, expiry and unknown commits without a matching durable receipt
return non-success. No provisioning, LLM turn, attachment fetch or platform reply
runs during acceptance. Types carrying bodies and credentials intentionally have
no `Debug` implementation; safe errors and receipts contain no body text or token.

## Recovery boundary and remaining integration

NyxID's pinned implementation persists inbound metadata, not generic callback
bodies, and has no generic body-redelivery guarantee after failure. CMA recovery
starts after durable acceptance. Pre-accept failure may lose the callback body;
unknown acceptance must be distinguished from a confirmed rejection. There is no
end-to-end lossless or exactly-once claim. Provider-native message identity is not
guaranteed by the generic schema and this slice does not claim secondary dedup.

Outstanding integration includes the public HTTP endpoint and its streaming body
limit, supported deployment configuration for pinned JWKS/TTL/callback URL,
isolated ingress scheduling, live selected-key custody composition, a worker
lifecycle, effect-specific authority checks and real platform verification.
Ordinary CMA bearer authentication continues to reject callback, relay and reply
credentials. No NyxID source, bot, callback configuration or live user was changed.

The focused core suite covers actual RSA signatures, two distinct signing keys,
rotation, exact-byte mutation, wrong claim/binding/header values, expiry, bounded
parsing, optional user corroboration and reply scope. Adapter tests cover duplicate
sensitive headers, expired/failed key caches and zero live provider calls for an
invalid attestation. Five ignored disposable Mongo
tests are registered under `bridge_definitions::mongo_integration_tests::inbox`
for duplicate races/restart, encrypted expiry, conflicts/configuration changes,
transactional persistence failure, capacity and revocation after verification.
Their execution is reported by
the consolidated Mongo contract campaign; registration alone is not a pass.
