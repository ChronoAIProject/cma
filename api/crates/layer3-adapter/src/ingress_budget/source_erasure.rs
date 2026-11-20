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
