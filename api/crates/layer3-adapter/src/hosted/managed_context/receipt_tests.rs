use super::*;
use crate::allocation_probe::AllocationProbe;

fn status() -> RuntimeStatus {
    RuntimeStatus {
        agent_browser: false,
        agent_filesystem: true,
        agent_attachment_upload: false,
        agent_skill_install: false,
        agent_skill_upgrade: false,
        agent_model_catalog: false,
        agent_resource_preparation: false,
        agent_process_restart: false,
        exact_github_credential_route: false,
        github_repository_scope: false,
        sandbox_id: "sandbox".into(),
        generation: 3,
        home_identity: "a".repeat(43),
        boot_id: "b".repeat(32),
        installation_ready: true,
        credential_ready: true,
        restore_pending: false,
        agent_processes: 1,
    }
}

fn legacy_receipt(
    binding: &HostedContextRefresh,
    status: &RuntimeStatus,
    session: &str,
    sequence: u64,
) -> String {
    let exact = serde_json::json!([
        "managed-context-v1",
        binding.0.handle.0,
        status.sandbox_id,
        status.generation,
        status.home_identity,
        status.boot_id,
        binding.0.path,
        session,
        sequence
    ]);
    format!("{:x}", Sha256::digest(exact.to_string().as_bytes()))
}

#[test]
fn receipt_hash_matches_the_existing_json_contract_for_every_field() {
    let (mut binding, _) = writer_tests::binding(0);
    let mut status = status();
    for text in ["plain", "quote\" slash\\", "\0\n\t", "世界🌍", ""] {
        let original = binding.receipt(&status, text, 1);
        binding.legacy_mut().handle.0 = text.into();
        binding.legacy_mut().path = text.repeat(4);
        status.sandbox_id = text.repeat(2);
        status.home_identity = text.repeat(3);
        status.boot_id = text.repeat(5);
        for sequence in [0, 1, u64::MAX] {
            status.generation = sequence;
            let receipt = binding.receipt(&status, text, sequence);
            assert_eq!(receipt, legacy_receipt(&binding, &status, text, sequence));
            assert_eq!(receipt.len(), 64);
            assert_ne!(receipt, original);
        }
    }
}

#[test]
fn isolated_receipt_hash_allocates_only_the_fixed_digest_result() {
    const TEST: &str = "hosted::managed_context::receipt_tests::isolated_receipt_hash_allocates_only_the_fixed_digest_result";
    if std::env::var_os("CMA_CONTEXT_RECEIPT_CHILD").is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST, "--nocapture", "--test-threads=1"])
            .env("CMA_CONTEXT_RECEIPT_CHILD", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        return;
    }
    let (mut binding, _) = writer_tests::binding(0);
    let mut status = status();
    binding.legacy_mut().handle.0 = "h".repeat(1024);
    binding.legacy_mut().path = "p".repeat(4096);
    status.sandbox_id = "s".repeat(256);
    let session = "\"\\世界🌍".repeat(128);
    let mut probe = AllocationProbe::new();
    let expected = probe.track(|| legacy_receipt(&binding, &status, &session, u64::MAX));
    eprintln!(
        "legacy_receipt peak={} retained={}",
        probe.peak(),
        probe.live()
    );
    assert!(probe.peak() > 64);
    drop(probe);

    let mut probe = AllocationProbe::new();
    let receipt = probe.track(|| binding.receipt(&status, &session, u64::MAX));
    eprintln!(
        "receipt peak={} retained={} allocated={}",
        probe.peak(),
        probe.live(),
        probe.allocated()
    );
    assert_eq!(receipt, expected);
    assert_eq!(probe.peak(), 64);
    assert_eq!(probe.live(), 64);
    assert_eq!(probe.allocated(), 64);
    drop(receipt);
    assert_eq!(probe.live(), 0);
}
