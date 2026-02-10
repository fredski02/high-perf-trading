use common::{Command, Event, OrderSnapshot};
use tokio::sync::oneshot;

#[derive(Debug)]
pub struct Inbound {
    pub conn_id: u64,
    pub cmd: Command,
}

#[derive(Debug)]
pub struct Outbound {
    pub conn_id: u64,
    pub ev: Event,
}

/// Query messages from gateway to engine (for reconciliation)
#[derive(Debug)]
pub enum EngineQuery {
    GetAllOrders {
        response_tx: oneshot::Sender<Vec<OrderSnapshot>>,
    },
}
