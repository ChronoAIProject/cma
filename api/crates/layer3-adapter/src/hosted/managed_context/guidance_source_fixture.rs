use super::*;
use crate::hosted::managed_context::receipt_admission::tests as status_fixture;
use crate::ingress_budget::{ClassBounds, IngressBudget};
use crate::wire::{AgentOpenReceipt, FencedReceipt, FencedRequest};
use crate::{AgentLaunch, ConversationChannel, ConversationUpdate, RuntimeObservation};
use futures_util::FutureExt;
use std::sync::{
    atomic::{AtomicUsize, Ordering::SeqCst},
    Mutex,
};

pub(in crate::hosted) struct Source {
    status: Arc<status_fixture::Source>,
    pub(in crate::hosted) text: Mutex<String>,
    pub(in crate::hosted) reads: AtomicUsize,
    pub(in crate::hosted) writes: AtomicUsize,
    pub(in crate::hosted) fault: u8,
    pub(in crate::hosted) foreign: Option<IngressOwnershipPolicy>,
}

#[async_trait::async_trait]
impl HostedBackend for Source {
    fn exchange_admitted<'a>(
        &'a self,
        handle: &'a RuntimeHandle,
        request: OwnedWork<RequestFrame>,
        policy: &IngressOwnershipPolicy,
    ) -> Result<AdmittedOperation<'a, ResponseFrame>, AdmittedError> {
        if matches!(&*request, RequestFrame::RuntimeStatus(_)) {
            return self.status.exchange_admitted(handle, request, policy);
        }
        assert!(request.belongs_to(policy));
        let owned_policy = policy.clone();
        AdmittedOperation::new(
            self.foreign.as_ref().unwrap_or(policy),
            IngressClass::Data,
            AllocationPlan::checked("synthetic file response", &[3 * MAX_INSTRUCTIONS, 1024])?,
            move |_| async move {
                match &*request {
                    RequestFrame::ReadAgentFile(read) => {
                        self.reads.fetch_add(1, SeqCst);
                        assert!(read.owner_access);
                        assert!(read.protected_paths.is_empty());
                        assert_eq!(read.sandbox_id, "synthetic-sandbox");
                        assert_eq!(read.runtime_generation, 3);
                        assert_eq!(read.boot_id.as_bytes(), &[b'a'; 32]);
                        assert_eq!(read.max_bytes, MAX_INSTRUCTIONS as u64);
                        if self.fault == 11 {
                            return Err(AdapterError::RuntimeLedgerHold("s".repeat(1024)));
                        }
                        let text = self.text.lock().unwrap();
                        let bytes = if self.fault == 12 {
                            &[0xff][..]
                        } else {
                            text.as_bytes()
                        };
                        let mut file = crate::AgentFileReadReceipt {
                            path: read.path.clone(),
                            size_bytes: bytes.len() as u64,
                            sha256: format!("{:x}", Sha256::digest(bytes)),
                            content_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
                        };
                        match self.fault {
                            4 => file.path.push_str(".other"),
                            5 => file.content_base64 = "invalid!".into(),
                            6 => file.sha256 = "b".repeat(64),
                            7 => file.size_bytes = MAX_INSTRUCTIONS as u64 + 1,
                            8 => {
                                file.content_base64 =
                                    "x".repeat(MAX_INSTRUCTIONS.div_ceil(3) * 4 + 1)
                            }
                            9 => {
                                return Ok(ResponseFrame::WriteAgentFile(
                                    crate::AgentFileWriteReceipt {
                                        path: read.path.clone(),
                                        sha256: "b".repeat(64),
                                        size_bytes: 0,
                                    },
                                ))
                            }
                            _ => {}
                        }
                        Ok(ResponseFrame::ReadAgentFile(file))
                    }
                    RequestFrame::WriteAgentFile(write) => {
                        self.writes.fetch_add(1, SeqCst);
                        // Transformation storage must have returned before a remote write can stall.
                        assert_eq!(
                            owned_policy
                                .budget(IngressClass::Transformation)
                                .available_bytes(),
                            owned_policy.bounds().transformation_bytes()
                        );
                        assert!(write.owner_access);
                        assert!(write.protected_paths.is_empty());
                        assert!(!write.directory);
                        assert_eq!(write.sandbox_id, "synthetic-sandbox");
                        assert_eq!(write.runtime_generation, 3);
                        assert_eq!(write.boot_id.as_bytes(), &[b'a'; 32]);
                        if self.fault == 10 {
                            std::future::pending::<()>().await;
                        }
                        let mut text = self.text.lock().unwrap();
                        if self.fault == 1 {
                            text.clear();
                            text.push_str("Concurrent owner edit");
                        }
                        if write.expected_sha256.as_deref()
                            != Some(format!("{:x}", Sha256::digest(text.as_bytes())).as_str())
                        {
                            return Err(AdapterError::Rejected("file changed concurrently".into()));
                        }
                        let content = write.content()?;
                        let content = std::str::from_utf8(&content).unwrap();
                        text.clear();
                        text.push_str(content);
                        if self.fault == 2 {
                            return Err(AdapterError::Unavailable("write ACK lost".into()));
                        }
                        Ok(ResponseFrame::WriteAgentFile(
                            crate::AgentFileWriteReceipt {
                                path: if self.fault == 13 {
                                    "other".into()
                                } else {
                                    write.path.clone()
                                },
                                sha256: if self.fault == 3 {
                                    "b".repeat(64)
                                } else {
                                    format!("{:x}", Sha256::digest(text.as_bytes()))
                                },
                                size_bytes: text.len() as u64 + u64::from(self.fault == 14),
                            },
                        ))
                    }
                    _ => panic!("unexpected file request"),
                }
            },
        )
    }
    async fn exchange(&self, _: &RuntimeHandle, _: RequestFrame) -> crate::Result<ResponseFrame> {
        panic!("admitted guidance cannot invoke a legacy producer")
    }
    async fn provision(
        &self,
        _: crate::provider::ProviderSandboxRequest,
    ) -> crate::Result<RuntimeHandle> {
        unreachable!()
    }
    async fn teardown(&self, _: &RuntimeHandle) -> crate::Result<()> {
        unreachable!()
    }
    async fn inventory(&self) -> crate::Result<Vec<RuntimeObservation>> {
        unreachable!()
    }
    async fn open_agent(
        &self,
        _: &RuntimeHandle,
        _: FencedRequest<AgentLaunch>,
        _: Option<String>,
        _: crate::hosted::HostedAgentOpenSettings,
        _: tokio::sync::mpsc::Sender<ConversationUpdate>,
    ) -> crate::Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        unreachable!()
    }
}

pub(in crate::hosted) fn binding(
    fault: u8,
    original: &str,
    foreign: Option<IngressOwnershipPolicy>,
) -> (HostedContextRefresh, Arc<Source>) {
    let (mut binding, status) = status_fixture::binding(
        if fault == 15 {
            1
        } else if fault == 16 {
            2
        } else {
            0
        },
        None,
    );
    let mut text = String::with_capacity(MAX_INSTRUCTIONS);
    text.push_str(original);
    let source = Arc::new(Source {
        status,
        text: Mutex::new(text),
        reads: AtomicUsize::new(0),
        writes: AtomicUsize::new(0),
        fault,
        foreign,
    });
    // Initialize fixture mutex backing before measuring the ingress operation.
    drop(source.text.lock().unwrap());
    Arc::get_mut(&mut binding.0).unwrap().backend = source.clone();
    (binding, source)
}

pub(in crate::hosted::managed_context) fn policy() -> (IngressBudget, IngressOwnershipPolicy) {
    let root = IngressBudget::new(4 * 1024 * 1024);
    let floor = AllocationPlan::checked("guidance fixture floor", &[1024 * 1024]).unwrap();
    let policy = IngressOwnershipPolicy::new(
        root.clone(),
        ClassBounds::new(root.capacity_bytes(), floor, floor).unwrap(),
    )
    .unwrap();
    for class in [
        IngressClass::Data,
        IngressClass::Transformation,
        IngressClass::Control,
    ] {
        drop(
            policy
                .admit(
                    class,
                    AllocationPlan::checked("policy backing", &[1]).unwrap(),
                )
                .unwrap(),
        );
    }
    (root, policy)
}

pub(in crate::hosted) fn receipt(
    binding: &HostedContextRefresh,
    policy: &IngressOwnershipPolicy,
) -> String {
    let receipt = binding
        .readiness_receipt_admitted("conversation", 1, policy)
        .unwrap()
        .now_or_never()
        .unwrap()
        .unwrap();
    receipt.as_ref().expect("ready fixture").clone()
}
