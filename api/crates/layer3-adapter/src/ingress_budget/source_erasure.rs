use super::SharedOwned;
use crate::hosted::HostedSubscriptionSource;

// Unsizing preserves the original allocation and its custody without a wrapper.
impl<T: HostedSubscriptionSource + 'static> From<SharedOwned<T>>
    for SharedOwned<dyn HostedSubscriptionSource>
{
    fn from(source: SharedOwned<T>) -> Self {
        Self {
            value: source.value,
            storage: source.storage,
        }
    }
}

macro_rules! erase_source {
    ($($source:path),+ $(,)?) => {$(
        impl<T: $source + 'static> From<SharedOwned<T>> for SharedOwned<dyn $source> {
            fn from(source: SharedOwned<T>) -> Self {
                Self { value: source.value, storage: source.storage }
            }
        }
    )+};
}

erase_source!(
    crate::hosted::HostedBackend,
    crate::hosted::HostedAccessToolSource,
    crate::hosted::HostedProviderToolSource,
);
