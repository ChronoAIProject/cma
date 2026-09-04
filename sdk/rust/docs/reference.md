# Reference

<!-- nav: 5 Reference -->

`Client::builder(base_url)` returns a `ClientBuilder`. The builder accepts an
optional request timeout and produces a cloneable `Client`.

`Client::read_conformance()` performs `GET /api/v1/conformance` anonymously and
returns `Conformance`, containing `service`, `status`, and the currently exposed
`interfaces`.

Resource methods are intentionally absent until issue #819 defines the
replacement public contract.
