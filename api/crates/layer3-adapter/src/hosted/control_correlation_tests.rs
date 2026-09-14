use super::*;
use crate::{
    CheckpointHoldReceipt, CheckpointHoldRequest, CheckpointReceipt, CheckpointRequest,
    CheckpointWriterResumeReceipt, CommandReceipt, InstallReceipt, InstallRequest, WriterState,
};

fn command() -> CommandReceipt {
    CommandReceipt {
        index: 0,
        exit_code: 0,
        stdout_sha256: "a".repeat(64),
        stderr_sha256: "b".repeat(64),
    }
}

#[test]
fn typed_install_reply_must_match_the_exact_replay_identity() {
    let request = InstallRequest::new("install-1", vec!["true".into()]);
    let frame = RequestFrame::Install(request.clone());
    let exact = ResponseFrame::Install(InstallReceipt {
        operation_id: request.operation_id.clone(),
        request_sha256: request.request_sha256.clone(),
        commands: vec![command()],
    });
    validate(&frame, &exact).unwrap();

    let mut changed = exact.clone();
    let ResponseFrame::Install(receipt) = &mut changed else {
        unreachable!()
    };
    receipt.operation_id = "install-2".into();
    assert!(validate(&frame, &changed).is_err());
}

#[test]
fn held_checkpoint_correlates_epoch_fence_and_constant_time_token() {
    let token = "c".repeat(64);
    let digest = "d".repeat(64);
    let request = RequestFrame::CheckpointForTeardown(CheckpointHoldRequest {
        checkpoint: CheckpointRequest {
            epoch: 9,
            exclusions: Vec::new(),
        },
        writer_resume_token: token.clone(),
    });
    let response = ResponseFrame::CheckpointForTeardown(CheckpointHoldReceipt {
        checkpoint: CheckpointReceipt {
            epoch: 9,
            object_reference: format!("home-{digest}.cma"),
            sha256: digest,
            writers: WriterState::Held,
        },
        writer_resume_token: token,
    });
    validate(&request, &response).unwrap();

    let mut changed = response;
    let ResponseFrame::CheckpointForTeardown(receipt) = &mut changed else {
        unreachable!()
    };
    receipt.writer_resume_token = "e".repeat(64);
    assert!(validate(&request, &changed).is_err());
}

#[test]
fn writer_resume_requires_a_positive_exact_acknowledgement() {
    let token = "f".repeat(64);
    let request = RequestFrame::ResumeCheckpointWriters(crate::CheckpointWriterResumeRequest {
        writer_resume_token: token.clone(),
    });
    let exact = ResponseFrame::ResumeCheckpointWriters(CheckpointWriterResumeReceipt {
        writer_resume_token: token,
        writers_resumed: true,
    });
    validate(&request, &exact).unwrap();

    let wrong_variant = ResponseFrame::CheckpointForTeardown(CheckpointHoldReceipt {
        checkpoint: CheckpointReceipt {
            epoch: 0,
            object_reference: String::new(),
            sha256: String::new(),
            writers: WriterState::Held,
        },
        writer_resume_token: String::new(),
    });
    assert!(validate(&request, &wrong_variant).is_err());
}
