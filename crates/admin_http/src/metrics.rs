use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

#[derive(Default)]
pub struct Metrics {
    // existing
    pub connections: AtomicU64,
    pub frames_in: AtomicU64,
    pub frames_out: AtomicU64,

    // NEW — engine behaviour
    pub fills_total: AtomicU64,
    pub rejects_total: AtomicU64,

    // gauge (can go up/down)
    pub engine_in_queue_depth: AtomicI64,
}

impl Metrics {
    // ---- helpers (nice to avoid repeating Ordering) ----

    #[inline]
    pub fn inc_connections(&self) {
        self.connections.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn inc_frames_in(&self) {
        self.frames_in.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn inc_frames_out(&self) {
        self.frames_out.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn inc_fills(&self) {
        self.fills_total.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn inc_rejects(&self) {
        self.rejects_total.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn queue_inc(&self) {
        self.engine_in_queue_depth.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn queue_dec(&self) {
        self.engine_in_queue_depth.fetch_sub(1, Ordering::Relaxed);
    }

    // ---- Prometheus text ----

    pub fn render_prom_text(&self) -> String {
        let c = self.connections.load(Ordering::Relaxed);
        let fi = self.frames_in.load(Ordering::Relaxed);
        let fo = self.frames_out.load(Ordering::Relaxed);

        let fills = self.fills_total.load(Ordering::Relaxed);
        let rejects = self.rejects_total.load(Ordering::Relaxed);
        let depth = self.engine_in_queue_depth.load(Ordering::Relaxed);

        format!(
            "\
            exchange_connections {}
            exchange_frames_in {}
            exchange_frames_out {}
            exchange_fills_total {}
            exchange_rejects_total {}
            exchange_engine_in_queue_depth {}
            ",
            c, fi, fo, fills, rejects, depth
        )
    }
}
