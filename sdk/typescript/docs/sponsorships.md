# TypeScript Sponsorships

<!-- nav: 65 Sponsorships -->

The SDK exposes exactly four sponsorship operations. All are authenticated,
bounded, secret-free page reads:

- `listSponsorshipCredentialsPage` reads credential metadata controlled by the
  authenticated owner.
- `listSponsorshipPoliciesPage` reads that owner's policies and durable accounting.
- `listContributorSponsorshipGrantsPage` reads grants attributed to the
  authenticated contributor.
- `listSponsorSponsorshipGrantsPage` reads grants controlled by the authenticated
  sponsor.

The same four methods are available on `Cma`. This package does not expose
sponsorship registration, rotation, revocation, policy mutation, acceptance,
activation, usage, or administration commands.

```ts
import { createClient } from '@chronoai/cma-sdk';

const cma = createClient({ baseUrl: 'https://cma.example.com' });

const credentials = await cma.listSponsorshipCredentialsPage({ limit: 25 });
const policies = await cma.listSponsorshipPoliciesPage({ limit: 25 });
const contributed = await cma.listContributorSponsorshipGrantsPage({ limit: 25 });
const sponsored = await cma.listSponsorSponsorshipGrantsPage({ limit: 25 });

void credentials;
void policies;
void contributed;
void sponsored;
```

## Paging and projection boundary

Start a collection with `limit`. When `next` is present, pass that opaque value
back through the matching method's `next` field. Credential and policy
continuations may contain only `limit` and `cursor`; grant continuations also carry
the exact `contributor` or `sponsor` role. The SDK rejects duplicate parameters,
unknown parameters, cross-role continuations, and a continuation for another
collection before issuing the next request.

Success bodies are decoded with exact field allowlists. Unknown root or nested
fields fail closed as `ProtocolError`, including provider authority digests,
operational aliases, stable or client request digests, pending operations, and
leases. Returned credential, policy, usage, grant, and inert capability projections
contain only their declared public fields. The SDK does not return provider
credentials, provider grant identifiers, revocation handles, or internal recovery
authority.

## Availability and custody

These methods establish a typed contract; they do not indicate that sponsorship is
enabled. CMA has a complete provider, billing, pricing, scheduler, worker,
runtime, and private-gateway composition path when a complete enabled deployment
tuple is supplied. The checked-in production configuration remains dormant and
kill-switched, with no sponsorship secret projection, so public sponsorship
requests remain unavailable until the reviewed evidence and provisioning for that
deployment are complete.

The Rust REST boundary copies each sponsorship JSON body from Axum's immutable
`Bytes` into application-owned zeroizing storage, drops the immutable transport
allocation immediately, and deserializes credential and replacement strings into
zeroizing parser-owned values. Only those application/parser-owned copies are
zeroized. Axum's original immutable `Bytes` allocation cannot be erased, and no
claim is made that upstream transport buffers are wiped. JavaScript strings and
buffers used by this read-only SDK likewise do not provide a reliable erasure
guarantee.
