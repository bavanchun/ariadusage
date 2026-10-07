use std::fmt;

use ariadusage_core::pipeline::FetchInteraction;
use tokio_util::sync::CancellationToken;

/// Shared execution context passed to every broker entry point.
#[derive(Clone)]
pub struct BrokerCall {
    pub interaction: FetchInteraction,
    pub cancel: CancellationToken,
    pub request_id: String,
}

impl fmt::Debug for BrokerCall {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BrokerCall")
            .field("interaction", &self.interaction)
            .field("request_id_len", &self.request_id.len())
            .field("cancelled", &self.cancel.is_cancelled())
            .finish()
    }
}
