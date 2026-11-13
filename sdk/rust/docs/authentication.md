# Authentication

<!-- nav: 3 Authentication -->

Configure resource credentials through `ClientBuilder::bearer_token`:

```rust,no_run
use cma_sdk::Client;

# fn run() -> Result<(), Box<dyn std::error::Error>> {
let client = Client::builder(std::env::var("CMA_BASE_URL")?)?
    .bearer_token(std::env::var("CMA_BEARER_TOKEN")?)?
    .build()?;
# Ok(())
# }
```

The SDK does not obtain, refresh, or persist credentials. Owner resource calls use
the configured token; discovery, signup choices, Trigger previews, and badges
always omit it, even on an authenticated client. A client without a token can access public discovery and marketplace
reads, while CMA enforces authorization on owner resources.

Do not place a credential in the deployment URL. `Client::builder` rejects URLs
containing user information so credentials cannot leak through debug output,
logs, or proxy request targets.

Bearer headers are marked sensitive, and `Debug` output only reports whether
authentication is configured. Redirects are not followed. Use HTTPS outside
localhost so transport encryption protects credentials. Do not place tokens in
profile environment variables, checked-in examples, logs, or error messages.

`auth().read_session()` returns `AuthSession::SignedOut` for the exact anonymous
401 document, and `SignedIn(Identity)` for an authenticated 200 document. A
503 `store_failed` is a dependency error, never a sign-out. Capabilities are
closed typed values; do not infer authorization from a display role.

```rust,no_run
use cma_sdk::{Client, auth::{AuthSession, LoginOptions}};
# async fn run(client: Client) -> Result<(), Box<dyn std::error::Error>> {
match client.auth().read_session().await? {
    AuthSession::SignedOut => {
        let target = client.auth().login_url(&LoginOptions {
            return_to: Some("/triggers/manage".into()), register: false,
        })?;
        println!("{target}");
    }
    AuthSession::SignedIn(identity) => println!("{}", identity.subject),
}
let signup = client.auth().read_signup_options().await?;
let logout = client.auth().logout_url()?;
# let _ = (signup, logout);
# Ok(())
# }
```

Login and logout helpers construct explicit browser navigation URLs only. The
OAuth callback belongs to browser authentication and is not an SDK token
exchange. Return targets must be canonical CMA workflow paths; absolute URLs,
fragments, unknown query strings, and arbitrary redirects are refused before transport.

## Renewable Sandbox Authority

New Sandbox admission requires renewable owner authority established through
CMA's public `/auth/login` OAuth flow with the same subject. A request bearer
authenticates a request; CMA does not persist it or assume it is renewable.
Launch creation and Start have the same prerequisite when they need a new
Sandbox. Reusing an already admitted Sandbox or Agent does not require new setup.
Default-context conversation creation also checks setup if it needs a Sandbox.

The normal HTTP problem document reports missing setup as status 409 and code
`sandbox_credential_setup_required`. Construct a browser destination with
`auth().login_url(...)`, finish sign-in with the same account, and retry the
definitively refused keyed operation with a **new** `IdempotencyKey`. Its original
key continues to replay the exact refusal after setup. For an ambiguous transport
failure, keep the original key and unchanged body instead.
Conversation creation is unkeyed: a definitive setup refusal can be retried after
sign-in, but an ambiguous creation failure is not automatically safe to replay.

`AuthSession::SignedIn` proves identity, not renewable authority. The next resource
admission checks setup again; store and provider outages remain dependency
errors. The SDK does not perform an OAuth token exchange or obtain refresh
credentials for a backend client.

Return destinations also support `/agent-manager`, `/event-gateway`, `/image-manager`,
Bot pages and the public actor-link completion path. A single `setup` query on
`/bots/new` and a single `tab` query on `/sandboxes/{id}` are preserved; duplicate
or unknown keys and fragments and raw non-ASCII bytes are rejected. Paths are limited to 256 bytes and
the complete encoded absolute login URL to 4,096 ASCII bytes (including origin and options). The anonymous `silent_sign_in` hint is
controlled by the API startup flag, default false; the SDK continues projecting
`SignedOut`. Logout is server-owned and emits separate session/suppression cookies.

Explicit preference recovery is backed by durable custody, with a separate
non-authenticating reference cookie. Same-origin logout revokes that proof;
cross-site logout revokes the session without changing the preference.

Browser binding cookies last 660 seconds: the ten-minute authorization attempt
plus a one-minute callback-processing margin. Attempt expiry is unchanged.
Predecessor attempts complete inline after reservation, and logout tombstones
last at least as long as the binding cookie. Neither SDK handles the OAuth code
exchange; browser cookies remain server-owned.

`api_commands` is optional on authenticated session responses for compatibility
with older CMA servers. Only `true` advertises command authority for the exact
verified bearer; false or absence grants nothing. It does not replace per-command
resource ACL, live provider verification or Sandbox owner credential setup.
