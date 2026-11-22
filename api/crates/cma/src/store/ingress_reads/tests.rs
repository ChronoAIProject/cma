use std::cell::Cell;

use layer2_core::sandbox::ingress_ownership::{
    sizing::shared_owner_bytes, AllocationPlan, ClassBounds, IngressBudget, IngressClass,
    IngressOwnershipPolicy,
};
use layer2_core::sandbox::store::SandboxStoreError;
use mongodb::bson::{doc, Document, RawDocumentBuf};

use super::layout::{BsonLayout, LayoutError};
use super::row::{decode, load, ConversionPlan, RowError, RowSchema};

thread_local! {
    static DECODE_CALLS: Cell<usize> = const { Cell::new(0) };
}

fn policy() -> (IngressBudget, IngressOwnershipPolicy) {
    let root = IngressBudget::new(1024 * 1024);
    let plan = |bytes| AllocationPlan::checked("row fixture", &[bytes]).unwrap();
    let policy = IngressOwnershipPolicy::new(
        root.clone(),
        ClassBounds::new(1024 * 1024, plan(16 * 1024), plan(768 * 1024)).unwrap(),
    )
    .unwrap();
    (root, policy)
}

fn raw(bytes: usize) -> RawDocumentBuf {
    RawDocumentBuf::try_from(&doc! {"value": "x".repeat(bytes)}).unwrap()
}

struct Text;

impl RowSchema for Text {
    type Value = String;
    const INVALID_RAW: &'static str = "invalid synthetic text row";

    fn plan(layout: BsonLayout) -> Result<ConversionPlan, LayoutError> {
        let retained_bytes =
            layout.domain_bytes(size_of::<String>(), shared_owner_bytes::<String>())?;
        Ok(ConversionPlan {
            temporary_bytes: layout.bson_bytes()? + retained_bytes + 1024,
            retained_bytes,
        })
    }

    fn decode(row: Document) -> Result<String, SandboxStoreError> {
        DECODE_CALLS.set(DECODE_CALLS.get() + 1);
        row.get_str("value")
            .map(str::to_owned)
            .map_err(|_| SandboxStoreError::Corrupt("missing synthetic text".into()))
    }
}

#[test]
fn successful_conversion_retains_only_data_until_the_final_shared_owner_drops() {
    let (root, policy) = policy();
    let raw = raw(8192);
    let value = decode::<Text>(&raw, &policy).unwrap();
    assert_eq!(*value, "x".repeat(8192));
    assert!(value.belongs_to(&policy));
    assert!(root.available_bytes() < root.capacity_bytes() - root.metadata_bytes());
    assert_eq!(
        policy
            .budget(IngressClass::Transformation)
            .available_bytes(),
        policy.bounds().transformation_bytes()
    );
    let retained = root.available_bytes();
    let shared = value.clone();
    assert!(shared.ptr_eq(&value));
    drop(value);
    drop(raw);
    assert_eq!(root.available_bytes(), retained);
    drop(shared);
    assert_eq!(
        root.available_bytes(),
        root.capacity_bytes() - root.metadata_bytes()
    );
}

#[test]
fn data_refusal_precedes_domain_conversion_and_releases_partial_work_admission() {
    let (root, policy) = policy();
    let occupied = policy
        .admit(
            IngressClass::Data,
            AllocationPlan::checked("occupied data", &[policy.bounds().data_bytes()]).unwrap(),
        )
        .unwrap();
    let held = root.available_bytes();
    DECODE_CALLS.set(0);
    assert!(matches!(
        decode::<Text>(&raw(8192), &policy),
        Err(RowError::Refused(LayoutError::Capacity))
    ));
    assert_eq!(DECODE_CALLS.get(), 0);
    assert_eq!(root.available_bytes(), held);
    let control = policy
        .admit(
            IngressClass::Control,
            AllocationPlan::checked("independent control", &[4096]).unwrap(),
        )
        .unwrap();
    drop(control);
    drop(occupied);
    assert_eq!(
        root.available_bytes(),
        root.capacity_bytes() - root.metadata_bytes()
    );
}

struct ExcessiveWorking;

impl RowSchema for ExcessiveWorking {
    type Value = String;
    const INVALID_RAW: &'static str = Text::INVALID_RAW;

    fn plan(layout: BsonLayout) -> Result<ConversionPlan, LayoutError> {
        Ok(ConversionPlan {
            temporary_bytes: 1024 * 1024,
            ..Text::plan(layout)?
        })
    }

    fn decode(row: Document) -> Result<String, SandboxStoreError> {
        Text::decode(row)
    }
}

#[test]
fn conversion_refusal_after_raw_preflight_never_calls_the_schema_decoder() {
    let (root, policy) = policy();
    DECODE_CALLS.set(0);
    assert!(matches!(
        decode::<ExcessiveWorking>(&raw(32), &policy),
        Err(RowError::Refused(LayoutError::Capacity))
    ));
    assert_eq!(DECODE_CALLS.get(), 0);
    assert_eq!(
        root.available_bytes(),
        root.capacity_bytes() - root.metadata_bytes()
    );
}

#[test]
fn each_conversion_plans_its_own_row_after_a_smaller_success() {
    let (root, policy) = policy();
    let small = decode::<Text>(&raw(32), &policy).unwrap();
    let held = root.available_bytes();
    let larger = raw(policy.bounds().data_bytes());
    DECODE_CALLS.set(0);
    assert!(matches!(
        decode::<Text>(&larger, &policy),
        Err(RowError::Refused(LayoutError::Capacity))
    ));
    assert_eq!(DECODE_CALLS.get(), 0);
    assert_eq!(root.available_bytes(), held);
    assert_eq!(*small, "x".repeat(32));
    drop(small);
    assert_eq!(
        root.available_bytes(),
        root.capacity_bytes() - root.metadata_bytes()
    );
}

#[test]
fn domain_error_keeps_its_original_class_and_work_owner_until_drop() {
    let (root, policy) = policy();
    let raw = RawDocumentBuf::try_from(&doc! {"unexpected": "synthetic"}).unwrap();
    let RowError::Domain(error) = decode::<Text>(&raw, &policy).unwrap_err() else {
        panic!("domain error lost its owner");
    };
    assert!(
        matches!(&*error, SandboxStoreError::Corrupt(message) if message == "missing synthetic text")
    );
    assert!(error.belongs_to(&policy));
    assert!(root.available_bytes() < root.capacity_bytes() - root.metadata_bytes());
    assert_eq!(
        policy.budget(IngressClass::Data).available_bytes(),
        policy.bounds().data_bytes()
    );
    drop(raw);
    assert!(root.available_bytes() < root.capacity_bytes() - root.metadata_bytes());
    drop(error);
    assert_eq!(
        root.available_bytes(),
        root.capacity_bytes() - root.metadata_bytes()
    );
}

#[test]
fn preflight_pressure_is_distinct_from_a_malformed_stored_row() {
    let (root, policy) = policy();
    let raw = RawDocumentBuf::from_bytes(vec![8, 0, 0, 0, 2, b'x', 0, 0]).unwrap();
    let occupied = policy
        .admit(
            IngressClass::Transformation,
            AllocationPlan::checked(
                "occupied conversion",
                &[policy.bounds().transformation_bytes()],
            )
            .unwrap(),
        )
        .unwrap();
    DECODE_CALLS.set(0);
    assert!(matches!(
        decode::<Text>(&raw, &policy),
        Err(RowError::Refused(LayoutError::Capacity))
    ));
    drop(occupied);
    assert!(matches!(
        decode::<Text>(&raw, &policy),
        Err(RowError::Refused(LayoutError::Structure))
    ));
    assert_eq!(DECODE_CALLS.get(), 0);
    assert_eq!(
        root.available_bytes(),
        root.capacity_bytes() - root.metadata_bytes()
    );
}

struct Panicking;

impl RowSchema for Panicking {
    type Value = String;
    const INVALID_RAW: &'static str = Text::INVALID_RAW;

    fn plan(layout: BsonLayout) -> Result<ConversionPlan, LayoutError> {
        Text::plan(layout)
    }

    fn decode(row: Document) -> Result<String, SandboxStoreError> {
        let _partial = Text::decode(row).unwrap();
        panic!("synthetic failure after partial domain allocation");
    }
}

#[test]
fn unwind_releases_partial_domain_conversion_and_both_reservations() {
    let (root, policy) = policy();
    let raw = raw(8192);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        decode::<Panicking>(&raw, &policy)
    }));
    assert!(result.is_err());
    assert_eq!(
        root.available_bytes(),
        root.capacity_bytes() - root.metadata_bytes()
    );
}

fn read_plan() -> AllocationPlan {
    AllocationPlan::checked("synthetic raw read", &[512 * 1024]).unwrap()
}

#[tokio::test]
async fn read_refusal_precedes_query_and_future_construction() {
    let (root, policy) = policy();
    let occupied = policy
        .admit(IngressClass::Transformation, read_plan())
        .unwrap();
    let before = root.available_bytes();
    let initialized = Cell::new(false);
    let result = load::<Text, _>(&policy, read_plan(), || {
        initialized.set(true);
        std::future::ready(Ok(Some(raw(32))))
    })
    .await;
    assert!(matches!(
        result,
        Err(RowError::Refused(LayoutError::Capacity))
    ));
    assert!(!initialized.get());
    assert_eq!(root.available_bytes(), before);
    drop(occupied);
    assert_eq!(
        root.available_bytes(),
        root.capacity_bytes() - root.metadata_bytes()
    );
}

#[tokio::test]
async fn read_retains_only_the_exact_converted_row_after_completion() {
    let (root, policy) = policy();
    let value = load::<Text, _>(&policy, read_plan(), || {
        assert!(root.available_bytes() < root.capacity_bytes() - root.metadata_bytes());
        std::future::ready(Ok(Some(raw(8192))))
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(*value, "x".repeat(8192));
    assert!(value.belongs_to(&policy));
    assert_eq!(
        policy
            .budget(IngressClass::Transformation)
            .available_bytes(),
        policy.bounds().transformation_bytes()
    );
    let held = root.available_bytes();
    let larger = load::<Text, _>(&policy, read_plan(), || {
        std::future::ready(Ok(Some(raw(policy.bounds().data_bytes()))))
    })
    .await;
    assert!(matches!(
        larger,
        Err(RowError::Refused(LayoutError::Capacity))
    ));
    assert_eq!(root.available_bytes(), held);
    assert_eq!(*value, "x".repeat(8192));
    drop(value);
    assert_eq!(
        root.available_bytes(),
        root.capacity_bytes() - root.metadata_bytes()
    );
}

#[tokio::test]
async fn absent_read_and_domain_failure_remain_distinct_and_owned() {
    let (root, policy) = policy();
    assert!(
        load::<Text, _>(&policy, read_plan(), || std::future::ready(Ok(None)))
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        root.available_bytes(),
        root.capacity_bytes() - root.metadata_bytes()
    );
    let pointer = Cell::new(std::ptr::null());
    let error = load::<Text, _>(&policy, read_plan(), || {
        let message = "synthetic read failure".repeat(128);
        pointer.set(message.as_ptr());
        std::future::ready(Err(SandboxStoreError::Unavailable(message)))
    })
    .await
    .unwrap_err();
    let RowError::Domain(error) = error else {
        panic!("read failure lost its domain category");
    };
    assert!(matches!(&*error, SandboxStoreError::Unavailable(message)
        if message.as_ptr() == pointer.get()));
    assert!(error.belongs_to(&policy));
    assert!(root.available_bytes() < root.capacity_bytes() - root.metadata_bytes());
    drop(error);
    assert_eq!(
        root.available_bytes(),
        root.capacity_bytes() - root.metadata_bytes()
    );
}

#[tokio::test]
async fn cancelling_a_pending_read_drops_its_future_before_returning_capacity() {
    struct Pending {
        root: IngressBudget,
        dropped: std::sync::Arc<std::sync::atomic::AtomicBool>,
        payload: Vec<u8>,
    }
    impl Drop for Pending {
        fn drop(&mut self) {
            assert!(
                self.root.available_bytes()
                    < self.root.capacity_bytes() - self.root.metadata_bytes()
            );
            drop(std::mem::take(&mut self.payload));
            assert!(
                self.root.available_bytes()
                    < self.root.capacity_bytes() - self.root.metadata_bytes()
            );
            self.dropped
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }
    impl std::future::Future for Pending {
        type Output = Result<Option<RawDocumentBuf>, SandboxStoreError>;
        fn poll(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Self::Output> {
            std::task::Poll::Pending
        }
    }
    let (root, policy) = policy();
    let dropped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let read = load::<Text, _>(&policy, read_plan(), || Pending {
            root: root.clone(),
            dropped: dropped.clone(),
            payload: vec![3; 8192],
        });
        assert_eq!(
            root.available_bytes(),
            root.capacity_bytes() - root.metadata_bytes()
        );
        tokio::pin!(read);
        assert!(futures_util::poll!(&mut read).is_pending());
        assert!(root.available_bytes() < root.capacity_bytes() - root.metadata_bytes());
    }
    assert!(dropped.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(
        root.available_bytes(),
        root.capacity_bytes() - root.metadata_bytes()
    );
}

#[tokio::test]
async fn unwinding_during_query_construction_returns_read_capacity() {
    use futures_util::FutureExt;
    let (root, policy) = policy();
    let read = load::<Text, std::future::Ready<_>>(&policy, read_plan(), || {
        assert!(root.available_bytes() < root.capacity_bytes() - root.metadata_bytes());
        panic!("synthetic query construction failure");
    });
    assert!(std::panic::AssertUnwindSafe(read)
        .catch_unwind()
        .await
        .is_err());
    assert_eq!(
        root.available_bytes(),
        root.capacity_bytes() - root.metadata_bytes()
    );
}

struct CopyOverlap;

impl RowSchema for CopyOverlap {
    type Value = String;
    const INVALID_RAW: &'static str = Text::INVALID_RAW;

    fn plan(layout: BsonLayout) -> Result<ConversionPlan, LayoutError> {
        Ok(ConversionPlan {
            temporary_bytes: 600 * 1024,
            ..Text::plan(layout)?
        })
    }

    fn decode(row: Document) -> Result<String, SandboxStoreError> {
        Text::decode(row)
    }
}

#[tokio::test]
async fn raw_buffer_spare_capacity_remains_charged_through_conversion_admission() {
    let (root, policy) = policy();
    DECODE_CALLS.set(0);
    let result = load::<CopyOverlap, _>(&policy, read_plan(), || {
        let mut bytes = Vec::with_capacity(256 * 1024);
        bytes.extend_from_slice(raw(32).as_bytes());
        std::future::ready(Ok(Some(RawDocumentBuf::from_bytes(bytes).unwrap())))
    })
    .await;
    assert!(matches!(
        result,
        Err(RowError::Refused(LayoutError::Capacity))
    ));
    assert_eq!(DECODE_CALLS.get(), 0);
    assert_eq!(
        root.available_bytes(),
        root.capacity_bytes() - root.metadata_bytes()
    );
}

#[tokio::test]
async fn raw_response_retains_spare_capacity_without_domain_conversion() {
    let (root, policy) = policy();
    let idle = root.available_bytes();
    let value = super::row::load_raw_with_error(&policy, read_plan(), || async {
        let mut bytes = raw(32).into_bytes();
        bytes.reserve_exact(64 * 1024);
        Ok::<_, SandboxStoreError>(Some(RawDocumentBuf::from_bytes(bytes).unwrap()))
    })
    .await
    .unwrap()
    .unwrap();
    assert!(value.requested_bytes() >= 64 * 1024 + shared_owner_bytes::<RawDocumentBuf>());
    let retained = root.available_bytes();
    let shared = value.clone();
    drop(value);
    assert_eq!(root.available_bytes(), retained);
    assert!(shared.belongs_to(&policy));
    drop(shared);
    assert_eq!(root.available_bytes(), idle);
}
