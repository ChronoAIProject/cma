//! One declaration drives compiled admission checks and publication evidence.

use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Versions {
    schema: String,
    runtime_contract: String,
    bootstrap: String,
    home_layout: String,
}

pub fn constants(source: &str) -> Result<String, &'static str> {
    if source.len() > 1024 {
        return Err("runtime compatibility declaration is oversized");
    }
    let value: Versions =
        serde_json::from_str(source).map_err(|_| "invalid runtime compatibility declaration")?;
    if value.schema != "cma-runtime-versions-v1" {
        return Err("unknown runtime compatibility schema");
    }
    for version in [
        &value.runtime_contract,
        &value.bootstrap,
        &value.home_layout,
    ] {
        let parsed: u16 = version.parse().map_err(|_| "invalid runtime version")?;
        if parsed == 0 || parsed.to_string() != *version {
            return Err("runtime version must be canonical and positive");
        }
    }
    Ok(format!(
        "pub const RUNTIME_CONTRACT_VERSION: &str = {:?};\n\
         pub const RUNTIME_BOOTSTRAP_VERSION: u16 = {};\n\
         pub const RUNTIME_BOOTSTRAP_VERSION_LABEL: &str = {:?};\n\
         pub const RUNTIME_HOME_LAYOUT_VERSION: &str = {:?};\n",
        value.runtime_contract, value.bootstrap, value.bootstrap, value.home_layout
    ))
}
