use super::{doc, policy, AllocationPlan, IngressClass, RawDocumentBuf, SandboxStoreError};
use crate::store::ingress_reads::{
    cursor::{DocumentSchema, Page, PageError, MAX_BYTES},
    error::{self, ReadFailure},
    layout::LayoutError,
    row::{self, RowError},
};
use layer2_core::sandbox::ingress_ownership::IngressWorkScope;

fn raw(bytes: usize) -> RawDocumentBuf {
    RawDocumentBuf::try_from(&doc! {"value":"x".repeat(bytes)}).unwrap()
}

#[test]
fn admitted_page_preserves_exact_canonical_byte_and_row_boundaries() {
    let (root, policy) = policy();
    let overhead = raw(0).as_bytes().len();
    let exact = raw(MAX_BYTES - overhead);
    let mut page = Page::new(&policy, 1).unwrap();
    page.push(&exact, &policy).unwrap();
    let result = page.finish();
    assert_eq!(result.len(), 1);
    assert_eq!(
        result[0].get_str("value").unwrap().len(),
        MAX_BYTES - overhead
    );
    drop(result);
    let mut page = Page::new(&policy, 2).unwrap();
    page.push(&raw(MAX_BYTES / 2 - overhead), &policy).unwrap();
    page.push(&raw(MAX_BYTES / 2 - overhead), &policy).unwrap();
    assert_eq!(page.finish().len(), 2);
    let mut page = Page::new(&policy, 2).unwrap();
    page.push(&raw(MAX_BYTES / 2), &policy).unwrap();
    assert!(matches!(
        page.push(&raw(MAX_BYTES / 2), &policy),
        Err(PageError::Invalid(
            "transaction page exceeds its byte budget"
        ))
    ));
    drop(page);
    let mut page = Page::new(&policy, 256).unwrap();
    let tiny = raw(0);
    for _ in 0..256 {
        page.push(&tiny, &policy).unwrap();
    }
    assert!(matches!(
        page.push(&tiny, &policy),
        Err(PageError::Invalid(
            "transaction page exceeds its row budget"
        ))
    ));
    drop(page);
    assert_eq!(
        root.available_bytes(),
        root.capacity_bytes() - root.metadata_bytes()
    );
}

#[test]
fn page_accounts_canonical_documents_with_repeated_bson_keys() {
    let (root, policy) = policy();
    // A repeated field collapses during Document conversion. The legacy page
    // limit counts that canonical representation, not the original raw bytes.
    let original = raw(MAX_BYTES / 2);
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&[0; 4]);
    bytes.extend_from_slice(&original.as_bytes()[4..original.as_bytes().len() - 1]);
    bytes.extend_from_slice(&raw(4).as_bytes()[4..]);
    let len = bytes.len() as i32;
    bytes[..4].copy_from_slice(&len.to_le_bytes());
    let duplicate = RawDocumentBuf::from_bytes(bytes).unwrap();
    let mut page = Page::new(&policy, 3).unwrap();
    for _ in 0..3 {
        page.push(&duplicate, &policy).unwrap();
    }
    let values = page.finish();
    assert_eq!(values.len(), 3);
    assert!(values
        .iter()
        .all(|value| value.get_str("value").unwrap() == "xxxx"));
    drop(values);
    assert_eq!(
        root.available_bytes(),
        root.capacity_bytes() - root.metadata_bytes()
    );
}

#[test]
fn page_refusal_preserves_prior_rows_and_independent_data_custody() {
    let (root, policy) = policy();
    let mut page = Page::new(&policy, 2).unwrap();
    page.push(&raw(8192), &policy).unwrap();
    let available = policy.budget(IngressClass::Data).available_bytes();
    let pressure = policy
        .admit(
            IngressClass::Data,
            AllocationPlan::checked("remaining Data", &[available]).unwrap(),
        )
        .unwrap();
    let held = root.available_bytes();
    assert!(matches!(
        page.push(&raw(32), &policy),
        Err(PageError::Row(RowError::Refused(LayoutError::Capacity)))
    ));
    assert_eq!(root.available_bytes(), held);
    drop(pressure);
    let values = page.finish();
    let first = values[0].clone();
    let held = root.available_bytes();
    drop(values);
    assert!(root.available_bytes() > held);
    assert!(root.available_bytes() < root.capacity_bytes() - root.metadata_bytes());
    assert_eq!(first.get_str("value").unwrap().len(), 8192);
    drop(first);
    assert_eq!(
        root.available_bytes(),
        root.capacity_bytes() - root.metadata_bytes()
    );
}

#[tokio::test]
async fn source_error_preserves_its_nonclone_payload_through_operation_composition() {
    let (root, policy) = policy();
    let address = std::cell::Cell::new(std::ptr::null());
    let result = row::load_with_error::<DocumentSchema, _, _>(
        &policy,
        AllocationPlan::checked("read error", &[64 * 1024]).unwrap(),
        || {
            let error = mongodb::error::Error::custom("synthetic".repeat(1024));
            address.set(std::ptr::from_ref(error.kind.as_ref()));
            std::future::ready(Err(error))
        },
    )
    .await;
    let storage = IngressWorkScope::new(
        policy
            .admit(
                IngressClass::Transformation,
                AllocationPlan::checked("result metadata", &[1024]).unwrap(),
            )
            .unwrap(),
    );
    let error = error::finish(result, &storage)
        .unwrap()
        .transpose()
        .unwrap_err();
    drop(storage);
    let ReadFailure::Driver(driver) = &*error else {
        panic!("driver error was erased")
    };
    assert_eq!(std::ptr::from_ref(driver.kind.as_ref()), address.get());
    assert!(driver
        .get_custom::<String>()
        .unwrap()
        .starts_with("synthetic"));
    assert!(error.belongs_to(&policy));
    assert!(root.available_bytes() < root.capacity_bytes() - root.metadata_bytes());
    drop(error);
    assert_eq!(
        root.available_bytes(),
        root.capacity_bytes() - root.metadata_bytes()
    );
}

#[test]
fn stored_corruption_and_allocation_refusal_have_different_error_channels() {
    let (root, policy) = policy();
    let storage = IngressWorkScope::new(
        policy
            .admit(
                IngressClass::Transformation,
                AllocationPlan::checked("result metadata", &[1024]).unwrap(),
            )
            .unwrap(),
    );
    let malformed = RawDocumentBuf::from_bytes(vec![8, 0, 0, 0, 2, b'x', 0, 0]).unwrap();
    let result = row::decode::<DocumentSchema>(&malformed, &policy).map_err(row::ReadError::Row);
    assert!(matches!(
        error::finish(result, &storage),
        Err(layer2_core::sandbox::ingress_ownership::AdmittedError::Unsupported(_))
    ));
    let diagnostic = storage
        .initialize_owned(|| SandboxStoreError::Corrupt("synthetic schema violation".into()));
    let result: Result<(), _> = Err(row::ReadError::Row(RowError::Domain(diagnostic)));
    let error = error::finish(result, &storage)
        .unwrap()
        .transpose()
        .unwrap_err();
    assert!(
        matches!(&*error, ReadFailure::Domain(SandboxStoreError::Corrupt(message))
        if message == "synthetic schema violation")
    );
    drop(error);
    let result: Result<(), _> = Err(row::ReadError::Row(RowError::Refused(
        LayoutError::Capacity,
    )));
    assert!(matches!(
        error::finish(result, &storage),
        Err(layer2_core::sandbox::ingress_ownership::AdmittedError::Refused(_))
    ));
    drop(storage);
    assert_eq!(
        root.available_bytes(),
        root.capacity_bytes() - root.metadata_bytes()
    );
}
