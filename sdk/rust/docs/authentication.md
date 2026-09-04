# Authentication

<!-- nav: 3 Authentication -->

The currently supported discovery request is public and sends no bearer
credential. Authenticated resource methods return with the replacement SDK
contract in issue #819.

Do not place a credential in the deployment URL. `Client::builder` rejects URLs
containing user information so credentials cannot leak through debug output,
logs, or proxy request targets.
