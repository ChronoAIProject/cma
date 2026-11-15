use super::*;
use layer2_core::sandbox::{SandboxHome, SandboxNativePool};
use layer3_adapter::agent_sandbox::{
    NativePoolClaim, NativePoolClaimRequest, NativePoolHomeOrigin,
};
use layer3_adapter::{HomeVolumeMode, HomeVolumePolicy, HomeVolumeReceipt};

fn native(base: &Sandbox, index: usize) -> Sandbox {
    let mut sandbox = base.clone();
    sandbox.id = SandboxId(format!("sbx_native_{index}"));
    sandbox.name = format!("native-{index}");
    let mut home = SandboxHome::reserve(
        &sandbox.id,
        HomeVolumePolicy {
            mode: HomeVolumeMode::NativeVctFullHomeV2,
            storage_class: "storage".into(),
            capacity_gib: 4,
            template_name: String::new(),
            template_generation: String::new(),
        },
    )
    .unwrap();
    let request = NativePoolClaimRequest {
        pool: crate::sandbox::worker_tests::native_pool_tests::Pools::binding(&home.request),
        home: home.request.clone(),
        runtime_generation: 1,
    };
    let claim = NativePoolClaim {
        request: request.clone(),
        name: format!("claim-{index}"),
        uid: format!("claim-uid-{index}"),
        resource_version: "1".into(),
        sandbox_name: Some(format!("member-{index}")),
    };
    home.native_pool = Some(Box::new(SandboxNativePool {
        request,
        claim: Some(claim.clone()),
        cleanup: None,
        cleaned: false,
    }));
    home.receipt = Some(HomeVolumeReceipt {
        native_authority: Some(Box::new(NativePoolHomeOrigin {
            claim,
            sandbox_uid: format!("member-uid-{index}"),
            pvc_uid: format!("pvc-uid-{index}"),
        })),
        volume_id: home.request.volume_id.clone(),
        request_sha256: home.request.computed_sha256(),
        storage_reference: format!("home-member-{index}"),
        storage_generation: format!("pvc-uid-{index}"),
    });
    home.validate(&sandbox.id).unwrap();
    sandbox.runtime.as_mut().unwrap().runtime_id =
        format!("agent-sandbox:member-{index}:member-uid-{index}");
    sandbox.lifecycle.home = Some(Box::new(home));
    sandbox.validate().unwrap();
    sandbox
}

#[tokio::test]
async fn startup_native_shared_pool_and_repeated_local_aliases_remain_independent() {
    let fixture = fixture(1, Some(34), None, Duration::from_millis(1)).await;
    let base = fixture.authority.values().next().unwrap();
    let authority = (0..34)
        .map(|index| {
            let row = native(base, index);
            (row.id.clone(), row)
        })
        .collect();
    assert_eq!(
        fixture
            .reconciler
            .startup_binding_schedule(&authority, &[], true),
        (8, "none")
    );
}

#[tokio::test]
async fn startup_aliases_include_suspended_handles_names_claims_and_homes() {
    let fixture = fixture(1, Some(34), None, Duration::from_millis(1)).await;
    let base = fixture.authority.values().next().unwrap();
    let first = native(base, 0);
    for mutation in 0..7 {
        let mut second = native(base, 1);
        let first_home = first.lifecycle.home.as_ref().unwrap();
        let first_receipt = first_home.receipt.as_ref().unwrap();
        let second_home = second.lifecycle.home.as_mut().unwrap();
        let receipt = second_home.receipt.as_mut().unwrap();
        match mutation {
            0 => second.lifecycle.suspended_runtime = first.runtime.clone().map(Box::new),
            // A changed UID must not hide a shared physical member name.
            1 => {
                second.runtime.as_mut().unwrap().runtime_id =
                    "agent-sandbox:member-0:different-uid".into()
            }
            2 => {
                second_home
                    .native_pool
                    .as_mut()
                    .unwrap()
                    .claim
                    .as_mut()
                    .unwrap()
                    .uid = "claim-uid-0".into()
            }
            3 => receipt.native_authority.as_mut().unwrap().claim.name = "claim-0".into(),
            4 => receipt
                .storage_reference
                .clone_from(&first_receipt.storage_reference),
            5 => receipt.native_authority.as_mut().unwrap().pvc_uid = "pvc-uid-0".into(),
            6 => second_home
                .request
                .volume_id
                .clone_from(&first_home.request.volume_id),
            _ => unreachable!(),
        }
        let authority = BTreeMap::from([
            (first.id.clone(), first.clone()),
            (second.id.clone(), second),
        ]);
        assert_eq!(
            fixture
                .reconciler
                .startup_binding_schedule(&authority, &[], true),
            (1, "overlapping_targets"),
            "case {mutation}"
        );
    }
}

#[tokio::test]
async fn startup_ambiguous_native_authority_and_inventory_select_serial() {
    let fixture = fixture(1, Some(34), None, Duration::from_millis(1)).await;
    let base = fixture.authority.values().next().unwrap();
    for mutation in 0..4 {
        let mut row = native(base, 0);
        match mutation {
            0 => row.runtime.as_mut().unwrap().runtime_id = "agent-sandbox:missing-uid".into(),
            1 => row.lifecycle.home = None,
            2 => row.lifecycle.home.as_mut().unwrap().receipt = None,
            3 => {
                row.lifecycle
                    .home
                    .as_mut()
                    .unwrap()
                    .native_pool
                    .as_mut()
                    .unwrap()
                    .claim = None
            }
            _ => unreachable!(),
        }
        let authority = BTreeMap::from([(row.id.clone(), row)]);
        assert_eq!(
            fixture
                .reconciler
                .startup_binding_schedule(&authority, &[], true),
            (1, "ambiguous_targets")
        );
    }
    let mut inventory = fixture.inventory.clone();
    inventory.extend(fixture.inventory.clone());
    assert_eq!(
        fixture
            .reconciler
            .startup_binding_schedule(&fixture.authority, &inventory, true),
        (1, "ambiguous_targets")
    );
}
