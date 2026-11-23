use std::{env, fs, path::PathBuf};

#[path = "build/runtime_versions.rs"]
mod runtime_versions;

fn main() {
    println!("cargo:rerun-if-changed=runtime-versions.json");
    println!("cargo:rerun-if-changed=build/runtime_versions.rs");
    let source = fs::read_to_string("runtime-versions.json")
        .expect("runtime compatibility declaration must be readable");
    let versions = runtime_versions::constants(&source)
        .expect("runtime compatibility declaration must be valid");
    let destination = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo must set OUT_DIR"));
    fs::write(destination.join("runtime_versions.rs"), versions)
        .expect("runtime compatibility constants must be written");
}
