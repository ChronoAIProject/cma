//! Input channel compatibility; production uses guarded queue entries.
use super::*;

/// An engine input, from a legacy raw channel or the measured scheduler channel.
pub struct Input<C>(Inner<C>);
enum Inner<C> {
    Raw(mpsc::Receiver<C>),
    Measured(Receiver<C>),
}
impl<C> From<mpsc::Receiver<C>> for Input<C> {
    fn from(receiver: mpsc::Receiver<C>) -> Self {
        Self(Inner::Raw(receiver))
    }
}
impl<C> From<Receiver<C>> for Input<C> {
    fn from(receiver: Receiver<C>) -> Self {
        Self(Inner::Measured(receiver))
    }
}
impl<C> Input<C> {
    pub async fn recv(&mut self) -> Option<C> {
        match &mut self.0 {
            Inner::Raw(receiver) => receiver.recv().await,
            Inner::Measured(receiver) => receiver.recv().await,
        }
    }
    pub fn close(&mut self) {
        match &mut self.0 {
            Inner::Raw(receiver) => receiver.close(),
            Inner::Measured(receiver) => receiver.close(),
        }
    }
}
