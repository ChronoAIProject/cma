# Rust Reference

<!-- nav: 80 Reference -->

The API reference is generated from the crate's rustdoc and types; signatures are
not copied into this guide. Generate and open it locally with:

```sh
cargo doc --no-deps --open
```

The crate is repository-only and carries `publish = false`, so it has no crates.io
release or docs.rs page. The integration files in this directory appear under
`cma_sdk::guides`, and their code fences run during `cargo test --doc`.

The public export list is in [`src/lib.rs`](../src/lib.rs). `Client` and
`ClientBuilder` own transport configuration, `EventStream` owns the reconnecting
stream, `types` are wire projections, and `Error` is the non-exhaustive failure
boundary. The crate intentionally does not expose its internal HTTP, decoding, or
stream-parser modules.

Unknown event discriminators are retained in `EventKind::Unknown`; malformed known
events and unknown session enum values fail as `Error::Protocol`. See
[Sessions](sessions.md) and [Streaming](streaming.md) for those compatibility rules.
