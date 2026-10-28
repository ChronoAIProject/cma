use super::HostedAdapter;
use crate::hosted::NonexecutingRuntimeProof;
use crate::{AdapterError, Result, RuntimeAdoptionRequest};

impl HostedAdapter {
    #[doc(hidden)]
    pub async fn suspend_nonexecuting_runtime(
        &self,
        request: &RuntimeAdoptionRequest,
        proof: &NonexecutingRuntimeProof,
    ) -> Result<()> {
        tokio::time::timeout(std::time::Duration::from_secs(150), async {
            let home = request
                .spec
                .home_volume
                .as_deref()
                .filter(|home| home.request.policy.is_native_vct())
                .ok_or_else(|| {
                    AdapterError::Rejected(
                        "nonexecuting runtime requires its original native home".into(),
                    )
                })?;
            home.validate_for(&request.spec.sandbox_id)?;
            self.verify_adoption_home_route(request)?;
            self.verify_home(home).await?;
            let rendered = self.retained_request(request, request.generation)?;
            self.backend
                .suspend_nonexecuting_runtime(&request.handle, &rendered, home, proof)
                .await?;
            self.states.lock().await.remove(&request.handle);
            self.verify_home(home).await
        })
        .await
        .map_err(|_| {
            AdapterError::Unavailable("nonexecuting runtime suspension timed out".into())
        })?
    }

    #[doc(hidden)]
    pub async fn inspect_nonexecuting_runtime(
        &self,
        request: &RuntimeAdoptionRequest,
    ) -> Result<Option<NonexecutingRuntimeProof>> {
        bounded(async {
            let Some(home) = request
                .spec
                .home_volume
                .as_deref()
                .filter(|home| home.request.policy.is_native_vct())
            else {
                return Ok(None);
            };
            home.validate_for(&request.spec.sandbox_id)?;
            self.verify_adoption_home_route(request)?;
            self.verify_home(home).await?;
            let rendered = self.retained_request(request, request.generation)?;
            let proof = self
                .backend
                .inspect_nonexecuting_runtime(&request.handle, &rendered, home)
                .await?;
            self.verify_home(home).await?;
            Ok(proof)
        })
        .await
    }

    #[doc(hidden)]
    pub async fn verify_nonexecuting_runtime(
        &self,
        request: &RuntimeAdoptionRequest,
        proof: &NonexecutingRuntimeProof,
    ) -> Result<()> {
        bounded(async {
            let home = request
                .spec
                .home_volume
                .as_deref()
                .filter(|home| home.request.policy.is_native_vct())
                .ok_or_else(|| {
                    AdapterError::Rejected(
                        "nonexecuting runtime requires its original native home".into(),
                    )
                })?;
            home.validate_for(&request.spec.sandbox_id)?;
            self.verify_adoption_home_route(request)?;
            self.verify_home(home).await?;
            let rendered = self.retained_request(request, request.generation)?;
            self.backend
                .verify_nonexecuting_runtime(&request.handle, &rendered, home, proof)
                .await?;
            self.verify_home(home).await
        })
        .await
    }
}

async fn bounded<T>(operation: impl std::future::Future<Output = Result<T>>) -> Result<T> {
    tokio::time::timeout(std::time::Duration::from_secs(30), operation)
        .await
        .map_err(|_| {
            AdapterError::Unavailable("nonexecuting runtime inspection timed out".into())
        })?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test(start_paused = true)]
    async fn nonexecuting_observation_deadline_bounds_the_entire_operation() {
        let start = tokio::time::Instant::now();
        let result = bounded(std::future::pending::<Result<()>>()).await;
        assert!(matches!(result, Err(AdapterError::Unavailable(_))));
        assert_eq!(start.elapsed(), std::time::Duration::from_secs(30));
    }
}
