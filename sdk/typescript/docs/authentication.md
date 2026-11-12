# Authentication

<!-- nav: 3 Authentication -->

Resource methods use browser cookies by default. Backend callers can disable
cookies and provide a rotation-aware credential callback; it is resolved for
each HTTP request. The callback should read from the host's credential store,
never from a URL or committed constant:

```ts
import { createClient } from '@chronoai/cma-sdk';

const cma = createClient({
  baseUrl: 'https://cma.example.com',
  cookies: false,
  credential: () => process.env.CMA_ACCESS_TOKEN,
});

await cma.agents.list();
```

Never place a credential in `baseUrl`; URLs containing user information are
rejected.

`readConformance` remains deliberately anonymous: it sends neither bearer
credentials nor cookies and does not invoke the credential callback. The SDK
does not perform an interactive sign-in, create API keys or store credentials.

`readTriggerPreview(config, publicId)` and `launches.preview(publicId)` are
credential-suppressed public reads, as is `auth.readSignupOptions()`. Admission requires the visitor's authenticated
identity; Launch reads, progress and connection handoffs are owner-checked.
Publisher methods require the separate Trigger-publishing grant. Operator methods
require their distinct operator grants; exposing a client namespace does not grant
authority to use it.

## Renewable Sandbox Authority

Creating a new Sandbox requires renewable owner authority established through
CMA's public `/auth/login` OAuth flow with the same subject. A request bearer
authenticates that request; CMA does not persist it or assume it can be renewed.
This prerequisite also applies to Launch creation or Start when resolution needs
a new Sandbox, but does not gate reuse of an already admitted Sandbox or Agent.
Default-context conversation creation also checks setup if it needs a Sandbox.

Missing setup returns HTTP 409 with `sandbox_credential_setup_required` in the
normal problem document. Use `auth.loginUrl({ returnTo })` for a supported CMA
workflow destination, complete sign-in with the same account, then submit the
refused keyed operation with a **new** idempotency key. The original key continues
to replay its exact refusal, even after setup. An ambiguous transport failure is
different: retry its original key and unchanged body to learn its outcome.
Conversation creation is unkeyed: retry a definitive setup refusal after sign-in,
but never assume an ambiguous conversation-creation failure is safe to replay.

A successful `auth.readSession()` proves identity, not renewable credential
readiness; resource admission checks that prerequisite again. Authentication
store or provider outages remain dependency errors, not instructions to replace
credentials. The SDK neither performs the OAuth exchange nor acquires a refresh
token for the application.

## Browser Sessions

`auth.readSession()` returns a discriminated `AuthSession`: HTTP 401 with
`authenticated: false` means signed out; HTTP 503 is an unavailable identity
store, not a signed-out result. Successful identities carry `subject`, explicit
`capabilities`, `visitor`, and optional display fields. Display fields are never
authorization evidence.

`auth.loginUrl({ returnTo, screen: 'register' })` validates a canonical CMA
workflow destination and returns the navigation URL without making a request.
`screen` is optional; `auth.logoutUrl()` similarly returns the logout navigation
URL without revoking anything until the application navigates there.
`auth.readSignupOptions()` supplies current registration availability.
Preserve an explicit visitor intent across sign-in,
with a bounded lifetime and its exact command key/body. An OAuth callback without
that intent must not create a Launch.

```ts
import { createClient } from '@chronoai/cma-sdk';

const cma = createClient();
const identity = await cma.auth.readSession();
if (!identity.authenticated) {
  const destination = cma.auth.loginUrl({ returnTo: '/triggers/manage' });
  console.log(destination);
}
```

`launches.createConnection` returns an opaque handoff id and a NyxID hosted URL,
never service credentials. Validate HTTPS and reject URL user information before
navigating. Treat the hosted URL as sensitive handoff data: do not log it or put it
in the return destination. Poll `readConnection` with the authenticated owner;
CMA does not inspect cross-origin window contents. Manual connection metadata
does not imply that a hosted handoff is available.

Browser return URLs preserve one `setup` query on `/bots/new` or one `tab` query
on `/sandboxes/{id}`. Paths remain canonical and relative (256 bytes); complete
encoded absolute login URLs are bounded to 4,096 ASCII bytes, including origin and options. Unknown/duplicate query keys and
fragments and raw non-ASCII bytes are refused. The startup-gated `silent_sign_in` hint defaults to false;
the SDK still projects `{ authenticated: false }`. Same-origin server logout records an opt-out time; successful explicit sign-in
records its attempt start separately and re-enables silent entry only when later.

Explicit preference recovery is backed by durable custody, with a separate
non-authenticating reference cookie. Same-origin logout revokes that proof;
cross-site logout revokes the session without changing the preference.

Browser binding cookies last 660 seconds: the ten-minute authorization attempt
plus a one-minute callback-processing margin. Attempt expiry is unchanged.
Predecessor attempts complete inline after reservation, and logout tombstones
last at least as long as the binding cookie. Neither SDK handles the OAuth code
exchange; browser cookies remain server-owned.

`MAX_LOGIN_URL_BYTES` is exported from the public package entry point. It is the
4,096-byte limit for the complete encoded absolute login URL, including its
origin and options. Reference clients should import it through the package alias:

```ts
import { MAX_LOGIN_URL_BYTES } from '@chronoai/cma-sdk';
```
