use common::{Command, Event};

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
