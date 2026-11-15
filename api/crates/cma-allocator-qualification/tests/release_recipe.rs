//! Both release recipes must execute the allocator in their shipped binary.
const API: &str = include_str!("../../../../infra/api/Dockerfile");
const GATEWAY: &str = include_str!("../../../../infra/cmaeg/Dockerfile");

fn qualified(recipe: &str, binary: &str) -> bool {
    let Some((_, build)) = recipe.split_once("FROM rust:1.96.1-alpine AS build\n") else {
        return false;
    };
    let Some((build, _)) = build.split_once("\nFROM ") else {
        return false;
    };
    let Some((_, runtime)) = recipe.rsplit_once(" AS runtime\n") else {
        return false;
    };
    let lines: Vec<_> = build.lines().map(str::trim).collect();
    let compile = lines
        .iter()
        .position(|line| *line == format!("cargo build --release --locked --bin {binary} \\"));
    let copy = lines
        .iter()
        .position(|line| *line == format!("&& cp /build/target/release/{binary} /out/{binary}"));
    let gate = lines.iter().position(|line| {
        *line == format!("RUN --network=none /out/{binary} --allocator-self-test")
    });
    matches!((compile, copy, gate), (Some(a), Some(b), Some(c)) if a < b && b < c)
        && !build.to_ascii_lowercase().contains("zig")
        && !runtime.contains("\nFROM ")
        && runtime
            .lines()
            .any(|line| line == format!("COPY --from=build /out/{binary} /usr/local/bin/{binary}"))
}

#[test]
fn api_and_gateway_qualify_the_actual_official_compiler_output() {
    assert!(qualified(API, "cma"));
    assert!(qualified(GATEWAY, "cmaeg"));
}

#[test]
fn missing_or_ignored_execution_cannot_qualify_the_api() {
    let gate = "RUN --network=none /out/cma --allocator-self-test";
    for replacement in [
        "",
        "RUN file /out/cma",
        "RUN --network=none /out/cma --allocator-self-test || true",
        "RUN --network=none /out/probe --allocator-self-test",
    ] {
        assert!(!qualified(&API.replace(gate, replacement), "cma"));
    }
}

#[test]
fn unqualified_linker_or_runtime_copies_cannot_qualify_the_api() {
    for (original, replacement) in [
        (
            "cargo build --release --locked --bin cma",
            "cargo zigbuild --release --locked --bin cma",
        ),
        (
            "cargo build --release --locked --bin cma",
            "RUSTC_LINKER=zig cargo build --release --locked --bin cma",
        ),
        (
            "COPY --from=build /out/cma /usr/local/bin/cma",
            "COPY --from=other /cma /usr/local/bin/cma",
        ),
    ] {
        assert!(!qualified(&API.replace(original, replacement), "cma"));
    }
    assert!(!qualified(
        &format!("{API}\nFROM unqualified AS preview\n"),
        "cma"
    ));
}

#[test]
fn the_gate_must_follow_the_final_compiler_output() {
    let gate = "RUN --network=none /out/cma --allocator-self-test";
    let early = API
        .replace(gate, "")
        .replace("WORKDIR /build", &format!("{gate}\nWORKDIR /build"));
    assert!(!qualified(&early, "cma"));
}
