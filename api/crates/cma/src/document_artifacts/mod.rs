//! Durable document metadata. Source objects are supplied by the independently
//! configured object repository; no document content is stored in this collection.
mod mongo;
pub use mongo::MongoDocumentStore;
pub(crate) use mongo::COLLECTION;

#[cfg(test)]
mod mongo_integration_tests;
