//! Only request telemetry opts into buffering; audits retain ActivityService.
use crate::{
    activity::{
        queue::{ActivityQueue, ActivityWorker},
        ActivityRecorder,
    },
    shutdown::ShutdownCoordinator,
};
use layer2_core::ActivityService;
use std::sync::Arc;

pub(super) fn compose(
    config: &crate::config::Config,
    service: &Arc<ActivityService>,
    stores: &crate::store::MongoStores,
) -> (Arc<ActivityRecorder>, Option<ActivityWorker>) {
    if config.activity_async_enabled {
        let (sink, worker) = ActivityQueue::new(stores.activity.clone());
        (
            ActivityRecorder::with_sink(service.clone(), sink),
            Some(worker),
        )
    } else {
        (ActivityRecorder::new(service.clone()), None)
    }
}

pub(crate) fn spawn(worker: Option<ActivityWorker>, shutdown: &mut ShutdownCoordinator) {
    if let Some(worker) = worker {
        shutdown.push(worker.spawn(shutdown.subscribe()));
    }
}
