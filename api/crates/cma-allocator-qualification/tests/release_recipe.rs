//! Both release recipes must execute the allocator in their shipped binary.
const API: &str = include_str!("../../../../api/crates/cma/Dockerfile");
const GATEWAY: &str = include_str!("../../../../api/crates/cmaeg/Dockerfile");
const RECIPES: [(&str, &str); 2] = [(API, "cma"), (GATEWAY, "cmaeg")];

fn logical_instructions(stage: &str) -> Option<Vec<String>> {
    let mut instructions = Vec::new();
    let mut pending = String::new();
    let mut heredoc: Option<(String, bool)> = None;
    for line in stage.lines() {
        if let Some((end, strip_tabs)) = &heredoc {
            let candidate = if *strip_tabs {
                line.trim_start_matches('\t')
            } else {
                line
            };
            if candidate == end {
                heredoc = None;
            }
            continue;
        }
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        if let Some(prefix) = line.trim_end().strip_suffix('\\') {
            pending.push_str(prefix);
            continue;
        }
        pending.push_str(line);
        if let Some((_, marker)) = pending.split_once("<<") {
            let marker = marker.trim();
            let strip_tabs = marker.starts_with('-');
            let marker = marker.strip_prefix('-').unwrap_or(marker);
            let end = marker.trim_matches(['\'', '"']);
            if end.is_empty() || !end.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
                return None;
            }
            heredoc = Some((end.into(), strip_tabs));
        }
        instructions.push(std::mem::take(&mut pending));
    }
    (pending.is_empty() && heredoc.is_none()).then_some(instructions)
}

fn compiles_and_copies(instruction: &str, binary: &str) -> bool {
    let Some(("RUN", command)) = instruction.trim().split_once(char::is_whitespace) else {
        return false;
    };
    let mut command = command.trim_start();
    while command.starts_with("--mount=") {
        let Some((_, rest)) = command.split_once(char::is_whitespace) else {
            return false;
        };
        command = rest.trim_start();
    }
    // The release RUN is a chain of plain commands. Reject other shell
    // syntax because it can skip compilation or swallow a failed copy.
    let steps: Vec<Vec<_>> = command
        .split("&&")
        .map(|step| step.split_whitespace().collect())
        .collect();
    if steps.iter().any(|step| {
        step.is_empty()
            || step
                .iter()
                .flat_map(|word| word.bytes())
                .any(|b| !b.is_ascii_alphanumeric() && !b"-_./=+,:%".contains(&b))
    }) {
        return false;
    }
    let prefix = ["cargo", "build", "--release", "--locked", "--bin", binary];
    let config = format!("profile.release.package.{binary}.codegen-units=256");
    let compile = steps.iter().position(|step| {
        step.strip_prefix(&prefix).is_some_and(|args| {
            args.strip_prefix(&["--verbose"]).unwrap_or(args) == ["--config", &config]
        })
    });
    let source = format!("/build/target/release/{binary}");
    let output = format!("/out/{binary}");
    let copy = steps
        .iter()
        .position(|step| *step == ["cp", &source, &output]);
    matches!((compile, copy), (Some(a), Some(b)) if a < b)
}

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
    let (Some(instructions), Some(runtime)) =
        (logical_instructions(build), logical_instructions(runtime))
    else {
        return false;
    };
    let compile = instructions
        .iter()
        .rposition(|line| compiles_and_copies(line, binary));
    let output = format!("/out/{binary}");
    let installation = format!("/usr/local/bin/{binary}");
    let gate = instructions.iter().position(|line| {
        line.split_whitespace()
            .eq(["RUN", "--network=none", &output, "--allocator-self-test"])
    });
    matches!((compile, gate), (Some(build), Some(test)) if build < test)
        && !build.to_ascii_lowercase().contains("zig")
        && !runtime
            .iter()
            .any(|line| line.trim_start().starts_with("FROM "))
        && runtime.iter().any(|line| {
            line.split_whitespace()
                .eq(["COPY", "--from=build", &output, &installation])
        })
}

#[test]
fn api_and_gateway_qualify_the_actual_official_compiler_output() {
    for (recipe, binary) in RECIPES {
        assert!(
            qualified(recipe, binary),
            "{binary} must qualify the official musl compiler output copied into its runtime"
        );
    }
}

#[test]
fn compilation_can_start_the_run_for_either_service() {
    for (_, binary) in RECIPES {
        let recipe = format!(
            "RUN cargo build --release --locked --bin {binary} \
             --config profile.release.package.{binary}.codegen-units=256 \
             && mkdir /out && cp /build/target/release/{binary} /out/{binary}"
        );
        assert!(compiles_and_copies(&recipe, binary));
    }
}

#[test]
fn compiler_bypass_and_missing_guards_are_rejected_in_both_recipes() {
    for (recipe, binary) in RECIPES {
        let compiler = format!("cargo build --release --locked --bin {binary}");
        let guarded = format!("&& {compiler}");
        let inline = recipe.replace(&guarded, &format!("&& true || {compiler}"));
        let continued = recipe.replace(&guarded, &format!("&& true || \\\n    {compiler}"));
        assert_ne!(recipe, inline);
        assert_ne!(recipe, continued);
        assert!(!qualified(&inline, binary), "{binary} inline bypass");
        assert!(!qualified(&continued, binary), "{binary} continued bypass");
        assert!(!qualified(&recipe.replace(&guarded, &compiler), binary));

        let copy = format!("&& cp /build/target/release/{binary} /out/{binary}");
        assert!(recipe.contains(&copy));
        assert!(!qualified(
            &recipe.replace(&copy, &copy.replacen("&&", "||", 1)),
            binary
        ));
        assert!(!qualified(
            &recipe.replace(&copy, &copy.replacen("&&", ";", 1)),
            binary
        ));
    }
}

#[test]
fn missing_or_ignored_execution_cannot_qualify_the_api() {
    let gate = "RUN --network=none /out/cma --allocator-self-test";
    for replacement in [
        "",
        "RUN file /out/cma",
        "RUN --network=none /out/cma --allocator-self-test || true",
        "RUN --network=none /out/probe --allocator-self-test",
        "RUN <<'EXAMPLE'\nRUN --network=none /out/cma --allocator-self-test\nEXAMPLE",
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
        (
            "COPY --from=build /out/cma /usr/local/bin/cma",
            "RUN <<'EXAMPLE'\nCOPY --from=build /out/cma /usr/local/bin/cma\nEXAMPLE",
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
