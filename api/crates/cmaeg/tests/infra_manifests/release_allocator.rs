use super::{stage, DOCKERFILE};

fn qualified_release(dockerfile: &str) -> bool {
    let build = stage(dockerfile, "build");
    let runtime = stage(dockerfile, "runtime");
    build.first() == Some(&"FROM rust:1.96.1-alpine AS build")
        && build.contains(&"cargo build --release --locked --bin cmaeg \\")
        && !build
            .iter()
            .any(|line| line.to_ascii_lowercase().contains("zig"))
        && build.contains(&"RUN --network=none /out/cmaeg --allocator-self-test")
        && runtime.contains(&"COPY --from=build /out/cmaeg /usr/local/bin/cmaeg")
        && dockerfile
            .lines()
            .rfind(|line| line.starts_with("FROM "))
            .is_some_and(|line| line.ends_with(" AS runtime"))
}

#[test]
fn release_build_qualifies_the_allocator_in_the_shipped_executable() {
    assert!(qualified_release(DOCKERFILE));
}

#[test]
fn a_zig_replacement_cannot_satisfy_the_release_build_contract() {
    let zig = DOCKERFILE.replace(
        "cargo build --release --locked --bin cmaeg",
        "cargo zigbuild --release --locked --bin cmaeg",
    );
    assert!(!qualified_release(&zig));
    let wrapper = DOCKERFILE.replace(
        "cargo build --release --locked --bin cmaeg",
        "RUSTC_LINKER=zig cargo build --release --locked --bin cmaeg",
    );
    assert!(!qualified_release(&wrapper));
}

#[test]
fn static_metadata_or_an_unqualified_copy_cannot_replace_execution() {
    let unchecked = DOCKERFILE.replace(
        "RUN --network=none /out/cmaeg --allocator-self-test",
        "RUN file /out/cmaeg",
    );
    assert!(!qualified_release(&unchecked));
    let replaced = DOCKERFILE.replace(
        "COPY --from=build /out/cmaeg /usr/local/bin/cmaeg",
        "COPY --from=local-binary /cmaeg /usr/local/bin/cmaeg",
    );
    assert!(!qualified_release(&replaced));
    let preview = format!("{DOCKERFILE}\nFROM local-preview AS unchecked\n");
    assert!(!qualified_release(&preview));
}
