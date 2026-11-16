//! Read the linked core's actual defaults without changing policy or spending credits.
//! This is a native component observation, not HTTP, Mongo or kind qualification.
use layer2_core::platform::PlatformSettings;
use layer2_core::sandbox::SandboxPolicy;
use serde_json::json;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let platform = PlatformSettings::default();
    platform.validate()?;
    let sandbox = SandboxPolicy::default();
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "schema": "cma-cost-defaults-v1",
            "scope": "NATIVE_CORE_DEFAULTS_ONLY",
            "platform_limits": platform.limits,
            "sandbox_owner_limit": sandbox.max_sandboxes_per_owner,
            "model_budget": platform.model_budget,
            "usd_valuation": null,
            "kind_runtime": false
        }))?
    );
    Ok(())
}
