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
fragments, query strings, and arbitrary redirects are refused before transport.
