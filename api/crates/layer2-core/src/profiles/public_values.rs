use async_trait::async_trait;

use super::EnvironmentVariable;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublicValueScanError {
    Secret { variable: String },
    Unavailable,
}

/// Deployment-owned detection policy for plain configuration made public.
#[async_trait]
pub trait PublicValueScanner: std::fmt::Debug + Send + Sync {
    async fn scan(&self, variables: &[EnvironmentVariable]) -> Result<(), PublicValueScanError>;
}

// Deterministic fixtures never invoke the production scanner or contain credentials.
#[cfg(any(test, feature = "test-support"))]
#[async_trait]
impl<const N: usize> PublicValueScanner for [String; N] {
    async fn scan(&self, variables: &[EnvironmentVariable]) -> Result<(), PublicValueScanError> {
        for variable in variables {
            if self
                .iter()
                .any(|pattern| !pattern.is_empty() && variable.value.contains(pattern))
            {
                return Err(PublicValueScanError::Secret {
                    variable: variable.name.clone(),
                });
            }
        }
        Ok(())
    }
}
