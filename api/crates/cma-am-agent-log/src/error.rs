use layer2_core::sandbox::store::SandboxStoreError;

#[derive(Debug)]
pub enum TransactionLogError {
    Domain(SandboxStoreError),
    Driver(mongodb::error::Error),
}

impl From<SandboxStoreError> for TransactionLogError {
    fn from(error: SandboxStoreError) -> Self {
        Self::Domain(error)
    }
}
