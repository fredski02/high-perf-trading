use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use bytes::{Bytes, BytesMut};
use codecs::Codec;
use common::{Event, Metrics, Reject, RejectReason};
use crossbeam_channel as cb;
use engine::Outbound;
use tokio::sync::mpsc;

pub type ConnTx = mpsc::Sender<Bytes>;

#[derive(Clone)]
struct ConnEntry {
    tx: ConnTx,
    codec: Arc<dyn Codec>,
}

/// Routes outbound events from engine to client connections
pub struct Router {
    conns: Mutex<HashMap<u64, ConnEntry>>,
    out_rx: cb::Receiver<Outbound>,
    metrics: Arc<Metrics>,
}

impl Router {
    pub fn new(out_rx: cb::Receiver<Outbound>, metrics: Arc<Metrics>) -> Self {
        Self {
            conns: Mutex::new(HashMap::new()),
            out_rx,
            metrics,
        }
    }

    /// Register a new connection
    pub fn register(&self, conn_id: u64, tx: ConnTx, codec: Arc<dyn Codec>) {
        self.conns
            .lock()
            .unwrap()
            .insert(conn_id, ConnEntry { tx, codec });
    }

    /// Unregister a connection
    pub fn unregister(&self, conn_id: u64) {
        self.conns.lock().unwrap().remove(&conn_id);
    }

    /// Run the router loop (blocking, runs in spawn_blocking)
    pub async fn run(self: Arc<Self>) {
        let this = self.clone();
        tokio::task::spawn_blocking(move || {
            while let Ok(out) = this.out_rx.recv() {
                let entry = this.conns.lock().unwrap().get(&out.conn_id).cloned();
                if let Some(ConnEntry { tx, codec }) = entry {
                    let mut payload = BytesMut::with_capacity(256);
                    if codec.encode_event(&out.ev, &mut payload).is_err() {
                        continue;
                    }
                    // LengthDelimitedCodec handles framing automatically
                    if tx.try_send(payload.freeze()).is_ok() {
                        this.metrics.inc_frames_out();
                    }
                }
            }
        })
        .await
        .ok();
    }

    /// Send an overloaded reject to a connection
    pub fn send_reject_overloaded(&self, conn_id: u64, client_seq: u64) {
        let entry = self.conns.lock().unwrap().get(&conn_id).cloned();
        if let Some(ConnEntry { tx, codec }) = entry {
            let ev = Event::Reject(Reject {
                server_seq: 0,
                client_seq,
                order_id: None,
                reason: RejectReason::Overloaded,
            });
            let mut payload = BytesMut::with_capacity(128);
            if codec.encode_event(&ev, &mut payload).is_ok() {
                // LengthDelimitedCodec handles framing automatically
                let _ = tx.try_send(payload.freeze());
            }
        }
    }
}