//! Head authority replaces timestamp selection only in explicitly managed mode.
use super::*;
use crate::release_authority::{ReleaseHead, ReleaseTarget};

impl MongoNativeWarmPools {
    pub(super) async fn admit_mutation(
        &self,
        kind: &str,
        payload: &impl serde::Serialize,
    ) -> Result<Option<Box<dyn layer3_adapter::effect_authority::EffectPermit>>, WarmPoolError>
    {
        use layer3_adapter::effect_authority::{EffectAuthority, EffectIntent};
        let Some(session) = &self.release_session else {
            return Ok(None);
        };
        let intent = EffectIntent::new(kind, &self.deployment, payload)
            .map_err(|_| WarmPoolError::Invalid)?;
        session
            .admit(&intent)
            .await
            .map(Some)
            .map_err(|_| WarmPoolError::Unavailable)
    }

    pub(crate) async fn open_managed(
        database: &Database,
        deployment: &str,
        session: std::sync::Arc<crate::runtime_release::Session>,
    ) -> Result<Self, StoreError> {
        crate::runtime_release::verify_pool_floor(database)
            .await
            .map_err(StoreError::Unavailable)?;
        if session.writer.deployment != deployment {
            return Err(StoreError::Unavailable(
                "foreign runtime release session".into(),
            ));
        }
        session.current().await.map_err(StoreError::Unavailable)?;
        Ok(Self {
            targets: majority_collection(database, COLLECTION),
            deployment: deployment.into(),
            database: database.clone(),
            release_session: Some(session),
        })
    }

    pub(super) async fn release_head(&self) -> Result<Option<ReleaseHead>, WarmPoolError> {
        match &self.release_session {
            Some(session) => session
                .current()
                .await
                .map(Some)
                .map_err(|_| WarmPoolError::Unavailable),
            None => Ok(None),
        }
    }

    pub(super) async fn activate_selected(&self, id: &str) -> Result<(), WarmPoolError> {
        let head = self
            .release_head()
            .await?
            .ok_or(WarmPoolError::Unavailable)?;
        let target = self.find(id).await?.ok_or(WarmPoolError::Conflict)?;
        let selected = if target.machine {
            head.pair.machine.as_ref().ok_or(WarmPoolError::Conflict)?
        } else {
            &head.pair.agent
        };
        if selected_matches(selected, &target) {
            Ok(())
        } else {
            Err(WarmPoolError::Conflict)
        }
    }
}

pub(super) fn selected_matches(selected: &ReleaseTarget, target: &PoolTarget) -> bool {
    crate::runtime_release::target_matches(selected, target)
}

pub(super) fn protected(head: &ReleaseHead, target: &PoolTarget) -> bool {
    head.protected_pairs.iter().any(|pair| {
        std::iter::once(&pair.agent)
            .chain(pair.machine.iter())
            .any(|selected| {
                selected.target_id == target.id
                    || selected.plan_sha256 == target.plan.config_sha256
                    || target.binding.as_ref().is_some_and(|binding| {
                        binding.uid == selected.pool_uid
                            || binding.template_uid == selected.template_uid
                    })
            })
    })
}
