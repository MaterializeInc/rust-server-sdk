use launchdarkly_server_sdk_evaluation::Reference;
use std::collections::HashSet;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Duration;

use self::sender::{EventSender, EventSenderResult};

pub mod dispatcher;
pub mod event;
pub mod processor;
pub mod processor_builders;
pub mod sender;

pub type OnEventSenderResultSuccess = Arc<dyn Fn(&EventSenderResult) + Send + Sync>;

pub struct EventsConfiguration {
    capacity: usize,
    event_sender: Arc<dyn EventSender>,
    flush_interval: Duration,
    context_keys_capacity: NonZeroUsize,
    context_keys_flush_interval: Duration,
    all_attributes_private: bool,
    private_attributes: HashSet<Reference>,
    omit_anonymous_contexts: bool,
    on_success: OnEventSenderResultSuccess,
}

#[cfg(test)]
fn create_events_configuration(
    event_sender: sender::InMemoryEventSender,
    flush_interval: Duration,
) -> EventsConfiguration {
    EventsConfiguration {
        capacity: 5,
        event_sender: Arc::new(event_sender),
        flush_interval,
        context_keys_capacity: NonZeroUsize::new(5).expect("5 > 0"),
        context_keys_flush_interval: Duration::from_secs(100),
        all_attributes_private: false,
        private_attributes: HashSet::new(),
        omit_anonymous_contexts: false,
        on_success: Arc::new(|_| ()),
    }
}

#[cfg(test)]
pub(super) fn create_event_sender() -> (
    sender::InMemoryEventSender,
    crossbeam_channel::Receiver<event::OutputEvent>,
) {
    let (event_tx, event_rx) = crossbeam_channel::unbounded();
    (sender::InMemoryEventSender::new(event_tx), event_rx)
}
