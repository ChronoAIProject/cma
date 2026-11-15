//! Conservative overlap detection; uncertainty retains the original serial path.
use std::collections::BTreeSet;

use layer2_core::sandbox::Sandbox;
use layer3_adapter::{agent_sandbox::NativePoolClaim, RuntimeHandle, RuntimeObservation};

type Aliases = BTreeSet<(&'static str, String)>;

pub(super) fn independent<'a>(
    sandboxes: impl Iterator<Item = &'a Sandbox>,
    inventory: &[RuntimeObservation],
) -> Result<Vec<RuntimeHandle>, &'static str> {
    let mut seen = Aliases::new();
    let mut handles = Vec::new();
    for sandbox in sandboxes {
        let mut owned = Aliases::new();
        let mut native = false;
        for binding in sandbox
            .runtime
            .iter()
            .chain(sandbox.lifecycle.suspended_runtime.as_deref())
        {
            let handle = RuntimeHandle::new(binding.runtime_id.clone());
            add(&mut owned, "runtime", handle.as_str())?;
            if let Some((name, uid)) = handle
                .native_member_identity()
                .map_err(|_| "ambiguous_targets")?
            {
                native = true;
                add(&mut owned, "native_member", name)?;
                add(&mut owned, "native_member_uid", uid)?;
            }
            let mut observations = inventory.iter().filter(|row| row.handle == handle);
            if let Some(row) = observations.next() {
                if observations.next().is_some()
                    || row.sandbox_id != sandbox.id.as_str()
                    || binding
                        .provision_generation
                        .is_some_and(|generation| generation != row.generation)
                {
                    return Err("ambiguous_targets");
                }
            }
            handles.push(handle);
        }
        if let Some(home) = &sandbox.lifecycle.home {
            add(&mut owned, "home_volume", &home.request.volume_id)?;
            if let Some(receipt) = &home.receipt {
                // Scope is deliberately not used to distinguish aliases: a false
                // collision loses overlap but cannot authorize concurrent mutation.
                add(&mut owned, "home_storage", &receipt.storage_reference)?;
                add(&mut owned, "home_storage_uid", &receipt.storage_generation)?;
                if let Some(origin) = &receipt.native_authority {
                    claim(&mut owned, &origin.claim)?;
                    add(&mut owned, "native_member_uid", &origin.sandbox_uid)?;
                    add(&mut owned, "home_storage_uid", &origin.pvc_uid)?;
                }
            } else if native {
                return Err("ambiguous_targets");
            }
            if let Some(pool) = &home.native_pool {
                // Transitional cleanup can retain additional provider aliases.
                // Preserve its previous ordering instead of interpreting them here.
                if pool.cleanup.is_some() {
                    return Err("ambiguous_targets");
                }
                if let Some(assigned) = &pool.claim {
                    claim(&mut owned, assigned)?;
                } else {
                    return Err("ambiguous_targets");
                }
            }
        } else if native {
            return Err("ambiguous_targets");
        }
        // Repeated references inside one validated Sandbox are normal. Only
        // another Sandbox's reference forces whole-initialization serialization.
        if owned.iter().any(|alias| seen.contains(alias)) {
            return Err("overlapping_targets");
        }
        seen.extend(owned);
    }
    Ok(handles)
}

fn add(aliases: &mut Aliases, kind: &'static str, value: &str) -> Result<(), &'static str> {
    if value.is_empty() || value.len() > 8_192 || value.chars().any(char::is_control) {
        return Err("ambiguous_targets");
    }
    aliases.insert((kind, value.to_owned()));
    Ok(())
}

fn claim(aliases: &mut Aliases, claim: &NativePoolClaim) -> Result<(), &'static str> {
    add(aliases, "native_claim", &claim.name)?;
    add(aliases, "native_claim_uid", &claim.uid)?;
    add(
        aliases,
        "native_member",
        claim.sandbox_name.as_deref().ok_or("ambiguous_targets")?,
    )
}
