//! Admitted application reads in the existing exclusive transaction.
use layer2_core::sandbox::ingress_ownership::{
    AdmittedError, AdmittedOperation, AllocationPlan, IngressClass, IngressOwnershipPolicy,
    SharedOwned,
};
use mongodb::{
    bson::{Document, RawDocumentBuf},
    Collection,
};

use crate::store::ingress_reads::{
    cursor::{Page, PageError, MAX_ROWS},
    error::{self, ReadFailure},
    row::{self, RowSchema},
};

use super::TransactionScope;

type RowOperation<'a, T> = AdmittedOperation<'a, Result<Option<SharedOwned<T>>, ReadFailure>>;
type PageOperation<'a> = AdmittedOperation<'a, Result<Vec<SharedOwned<Document>>, ReadFailure>>;

impl TransactionScope<'_> {
    /// `read_plan` includes query/schema rewriting, raw response and driver error
    /// storage. Driver bounds/cleanup remain unqualified; the enabled recorder
    /// keeps refusing until that capability is supplied.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "enabled recording awaits driver resource custody")
    )]
    pub(in crate::store) fn find_one_admitted<'a, S, I>(
        &'a mut self,
        collection: &'a Collection<Document>,
        policy: &'a IngressOwnershipPolicy,
        read_plan: AllocationPlan,
        initialize: I,
    ) -> Result<RowOperation<'a, S::Value>, AdmittedError>
    where
        S: RowSchema,
        S::Value: Send + Sync + 'a,
        I: FnOnce() -> Document + Send + 'a,
    {
        AdmittedOperation::from_owned(
            policy,
            IngressClass::Transformation,
            AllocationPlan::checked("transaction row operation", &[])?,
            move |scope| async move {
                let result = row::load_with_error::<S, _, _>(policy, read_plan, || async {
                    collection
                        .clone_with_type::<RawDocumentBuf>()
                        .find_one(crate::store::work_schema::for_collection(
                            collection,
                            initialize(),
                        ))
                        .session(&mut *self.session)
                        .await
                })
                .await;
                error::finish(result, &scope)
            },
        )
    }

    /// Retain the bounded page and each converted row independently of the
    /// operation. The original session stays borrowed through final cursor use.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "enabled recording awaits driver resource custody")
    )]
    pub(in crate::store) fn find_page_admitted<'a, I>(
        &'a mut self,
        collection: &'a Collection<Document>,
        policy: &'a IngressOwnershipPolicy,
        read_plan: AllocationPlan,
        limit: u32,
        initialize: I,
    ) -> Result<PageOperation<'a>, AdmittedError>
    where
        I: FnOnce() -> (Document, Document) + Send + 'a,
    {
        AdmittedOperation::from_owned(
            policy,
            IngressClass::Transformation,
            read_plan,
            move |scope| async move {
                if !(1..=MAX_ROWS).contains(&limit) {
                    return Ok(scope.initialize_owned(|| {
                        Err(ReadFailure::InvalidPage(
                            "transaction page limit must be 1..=256",
                        ))
                    }));
                }
                let mut page = Page::new(policy, limit)?;
                let (filter, sort) = initialize();
                let mut cursor = match collection
                    .clone_with_type::<RawDocumentBuf>()
                    .find(crate::store::work_schema::for_collection(
                        collection, filter,
                    ))
                    .sort(sort)
                    .limit(i64::from(limit))
                    .batch_size(limit)
                    .session(&mut *self.session)
                    .await
                {
                    Ok(cursor) => cursor,
                    Err(error) => {
                        return Ok(scope.initialize_owned(|| Err(ReadFailure::Driver(error))))
                    }
                };
                while let Some(row) = cursor.next(&mut *self.session).await {
                    let raw = match row {
                        Ok(raw) => raw,
                        Err(error) => {
                            return Ok(scope.initialize_owned(|| Err(ReadFailure::Driver(error))))
                        }
                    };
                    match page.push(&raw, policy) {
                        Ok(()) => {}
                        Err(PageError::Invalid(reason)) => {
                            return Ok(
                                scope.initialize_owned(|| Err(ReadFailure::InvalidPage(reason)))
                            )
                        }
                        Err(PageError::Row(error)) => {
                            return error::finish(Err(row::ReadError::Row(error)), &scope)
                        }
                    }
                }
                Ok(page.finish().map(Ok))
            },
        )
    }
}
