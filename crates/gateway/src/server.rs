use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};

use admin_http::metrics::Metrics;
use anyhow::Context;
use bytes::{Buf, BufMut, Bytes, BytesMut};
use codecs::{BinaryCodec, Codec, JsonCodec};
use common::{Event, ProtoError, Reject, RejectReason};
use crossbeam_channel as cb;
use engine::{Engine, Inbound, Outbound};
use persistence::Journal;
use tokio::net::tcp::OwnedReadHalf;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::mpsc,
};
use tracing::{info, warn};

static NEXT_CONN_ID: AtomicU64 = AtomicU64::new(1);

type ConnTx = mpsc::Sender<Bytes>;

struct ConnEntry {
    tx: ConnTx,
    codec: Arc<dyn Codec>,
}

struct Router {
    conns: Mutex<HashMap<u64, ConnEntry>>,
    out_rx: cb::Receiver<Outbound>,
    #[allow(dead_code)]
    max_frame: usize,
    metrics: Arc<Metrics>,
}

impl Router {
    fn new(out_rx: cb::Receiver<Outbound>, max_frame: usize, metrics: Arc<Metrics>) -> Self {
        Self {
            conns: Mutex::new(HashMap::new()),
            out_rx,
            max_frame,
            metrics,
        }
    }

    fn register(&self, conn_id: u64, tx: ConnTx, codec: Arc<dyn Codec>) {
        self.conns
            .lock()
            .unwrap()
            .insert(conn_id, ConnEntry { tx, codec });
    }

    fn unregister(&self, conn_id: u64) {
        self.conns.lock().unwrap().remove(&conn_id);
    }

    async fn run(self: Arc<Self>) {
        let this = self.clone();
        tokio::task::spawn_blocking(move || {
            while let Ok(out) = this.out_rx.recv() {
                let entry = this.conns.lock().unwrap().get(&out.conn_id).cloned();
                if let Some(ConnEntry { tx, codec }) = entry {
                    let mut payload = BytesMut::with_capacity(256);
                    if codec.encode_event(&out.ev, &mut payload).is_err() {
                        continue;
                    }
                    let framed = frame_payload(payload.freeze());
                    if tx.try_send(framed).is_ok() {
                        this.metrics.inc_frames_out();
                    }
                }
            }
        })
        .await
        .ok();
    }

    fn send_reject_overloaded(&self, conn_id: u64, client_seq: u64) {
        let entry = self.conns.lock().unwrap().get(&conn_id).cloned();
        if let Some(ConnEntry { tx, codec }) = entry {
            let ev = Event::Reject(Reject {
                server_seq: 0, // placeholder; in a real design engine assigns server_seq
                client_seq,
                order_id: None,
                reason: RejectReason::Overloaded,
            });
            let mut payload = BytesMut::with_capacity(128);
            if codec.encode_event(&ev, &mut payload).is_ok() {
                let framed = frame_payload(payload.freeze());
                let _ = tx.try_send(framed);
            }
        }
    }
}

impl Clone for ConnEntry {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            codec: self.codec.clone(),
        }
    }
}

pub async fn run(args: crate::Args) -> anyhow::Result<()> {
    let journal = Arc::new(Mutex::new(Journal::open(&args.journal_path)?));
    let replay_cmds = {
        let mut j = journal.lock().unwrap();
        j.read_all()?
    };

    let metrics = Arc::new(Metrics::default());

    // spawn admin_http in-process
    let admin_addr = args.admin_addr.clone();
    let metrics_admin = metrics.clone();
    tokio::spawn(async move {
        if let Err(e) = admin_http::server::run(admin_addr, metrics_admin).await {
            tracing::warn!("admin_http failed: {e:#}");
        }
    });

    let (in_tx, in_rx) = cb::bounded::<Inbound>(args.ingress_cap);
    let (out_tx, out_rx) = cb::unbounded::<Outbound>();

    // ✅ engine thread
    let metrics_r = metrics.clone();
    std::thread::Builder::new()
        .name("engine".into())
        .spawn(move || {
            let mut e = Engine::new(in_rx, out_tx, metrics_r);
            e.replay(replay_cmds);
            e.run();
        })
        .context("spawn engine")?;

    // router uses the same metrics
    let router = Arc::new(Router::new(out_rx, args.max_frame, metrics.clone()));
    tokio::spawn({
        let r = router.clone();
        async move { r.run().await }
    });

    // listeners need journal too
    let bin_codec: Arc<dyn Codec> = Arc::new(BinaryCodec);
    let json_codec: Arc<dyn Codec> = Arc::new(JsonCodec);

    tokio::spawn(run_listener(
        args.binary_addr.clone(),
        bin_codec,
        in_tx.clone(),
        router.clone(),
        metrics.clone(),
        args.max_frame,
        journal.clone(),
    ));

    tokio::spawn(run_listener(
        args.json_addr.clone(),
        json_codec,
        in_tx.clone(),
        router.clone(),
        metrics.clone(),
        args.max_frame,
        journal.clone(),
    ));

    tokio::signal::ctrl_c().await?;
    Ok(())
}

async fn run_listener(
    addr: String,
    codec: Arc<dyn Codec>,
    engine_in: cb::Sender<Inbound>,
    router: Arc<Router>,
    metrics: Arc<Metrics>,
    max_frame: usize,
    journal: Arc<Mutex<Journal>>,
) -> anyhow::Result<()> {
    let listener = TcpListener::bind(&addr).await?;
    info!("listening {} ({})", addr, codec.name());

    loop {
        let (socket, _peer) = listener.accept().await?;
        socket.set_nodelay(true).ok();
        metrics.inc_connections();

        let conn_id = NEXT_CONN_ID.fetch_add(1, Ordering::Relaxed);

        let (wtx, mut wrx) = mpsc::channel::<Bytes>(4096);
        router.register(conn_id, wtx, codec.clone());

        // split stream into owned read/write halves
        let (rd, mut wr) = socket.into_split();

        let router_w = router.clone();
        tokio::spawn(async move {
            while let Some(frame) = wrx.recv().await {
                if wr.write_all(&frame).await.is_err() {
                    break;
                }
            }
            router_w.unregister(conn_id);
        });

        let router_r = router.clone();
        let codec_r = codec.clone();
        let engine_in_r = engine_in.clone();
        let metrics_r = metrics.clone();
        let journal_r = journal.clone();

        tokio::spawn(async move {
            if let Err(_e) = read_loop(
                rd,
                conn_id,
                codec_r,
                engine_in_r,
                router_r,
                metrics_r,
                max_frame,
                journal_r,
            )
            .await
            {
                // keep quiet for now
            }
        });
    }
}

#[allow(clippy::too_many_arguments)]
async fn read_loop(
    mut rd: OwnedReadHalf,
    conn_id: u64,
    codec: Arc<dyn Codec>,
    engine_in: cb::Sender<Inbound>,
    router: Arc<Router>,
    metrics: Arc<Metrics>,
    max_frame: usize,
    journal: Arc<Mutex<Journal>>,
) -> Result<(), ProtoError> {
    let mut buf = BytesMut::with_capacity(16 * 1024);
    let mut temp = [0u8; 8192];

    loop {
        let n = rd
            .read(&mut temp)
            .await
            .map_err(|_| ProtoError::Malformed("io"))?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&temp[..n]);
        while let Some(payload) = try_read_frame(&mut buf, max_frame)? {
            metrics.inc_frames_in();

            let cmd = match codec.decode_command(&payload) {
                Ok(c) => c,
                Err(_) => continue,
            };

            if engine_in.try_send(Inbound { conn_id, cmd }).is_ok() {
                metrics.queue_inc();

                // ✅ journal only commands that made it into the engine queue
                if let Ok(mut j) = journal.lock() {
                    if let Err(e) = j.append(&cmd) {
                        warn!("journal append failed: {e:#}");
                    }
                }
            } else {
                let client_seq = match &cmd {
                    common::Command::NewOrder(x) => x.client_seq,
                    common::Command::Cancel(x) => x.client_seq,
                    common::Command::Replace(x) => x.client_seq,
                };
                router.send_reject_overloaded(conn_id, client_seq);
            }
        }
    }

    router.unregister(conn_id);
    Ok(())
}
fn try_read_frame(buf: &mut BytesMut, max_frame: usize) -> Result<Option<Bytes>, ProtoError> {
    if buf.len() < 4 {
        return Ok(None);
    }
    let len = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    if len > max_frame {
        return Err(ProtoError::FrameTooLarge(len));
    }
    if buf.len() < 4 + len {
        return Ok(None);
    }
    buf.advance(4);
    Ok(Some(buf.split_to(len).freeze()))
}

fn frame_payload(payload: Bytes) -> Bytes {
    let mut out = BytesMut::with_capacity(4 + payload.len());
    out.put_u32_le(payload.len() as u32);
    out.extend_from_slice(&payload);
    out.freeze()
}
