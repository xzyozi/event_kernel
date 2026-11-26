//! テスト用の模擬ポートと戦略。どの操作も、ステップ中にヒープを確保しない
//! （`Vec` は作成時に容量を確保し、容量の内側でだけ追加する）。
#![allow(dead_code)]

use ek_core::*;

pub const PAYLOAD: u32 = 64;

pub fn config() -> KernelConfig {
    KernelConfig {
        history_ring_len: 4,
        arena_size: 64,
        strategy_arena_offset: 0,
        strategy_arena_size: 64,
        action_queue_capacity: 16,
        payload_record_size: PAYLOAD,
        price_scale_exp: -2,
        qty_scale_exp: 0,
        prng_seed: 1,
        exec_state_max_bytes: 64,
        feed_token_max_bytes: 16,
        hash_interval: 0,
        trace_enabled: false,
        max_total_bytes: 1 << 20,
    }
}

pub fn tick(timestamp_ns: i64, sequence_id: u64) -> EventHeader {
    EventHeader {
        timestamp_ns,
        sequence_id,
        stream_id: 0,
        event_type: event_type::TICK,
        flags: 0,
        payload_idx: 0,
    }
}

/// メモリ上のイベント列を返すフィード。
pub struct VecFeed {
    pub events: Vec<EventHeader>,
    pub next: usize,
    pub fail_at: Option<usize>,
}

impl VecFeed {
    pub fn new(events: Vec<EventHeader>) -> Self {
        Self {
            events,
            next: 0,
            fail_at: None,
        }
    }

    pub fn ticks(timestamps: &[i64]) -> Self {
        Self::new(
            timestamps
                .iter()
                .enumerate()
                .map(|(i, &t)| tick(t, i as u64))
                .collect(),
        )
    }
}

impl EventFeedPort for VecFeed {
    fn peek_next_timestamp(&mut self) -> Result<Option<i64>, PortError> {
        if self.fail_at == Some(self.next) {
            return Err(PortError(7));
        }
        Ok(self.events.get(self.next).map(|e| e.timestamp_ns))
    }

    fn pop_next(&mut self, header: &mut EventHeader, payload: &mut [u8]) -> Result<(), PortError> {
        let event = *self.events.get(self.next).ok_or(PortError(8))?;
        *header = event;
        payload[..8].copy_from_slice(&event.timestamp_ns.to_le_bytes());
        self.next += 1;
        Ok(())
    }
}

/// 呼び出しの記録。`(種別, 時刻, 履歴の長さ, 履歴の最新の時刻)`。
pub type Call = (u16, i64, usize, Option<i64>);

/// 到達したアクションを `OrderAck` で受け付け、次の市場イベントで `Fill` にする。
pub struct FillNextExec {
    pub calls: Vec<Call>,
    pending: u32,
    results: [(u16, i64); 8],
    result_len: usize,
    result_pos: usize,
    /// 結果の `event_type` と時刻の差（不正な結果を返すテスト用）。
    pub ack_type: u16,
    pub ack_time_offset: i64,
}

impl FillNextExec {
    pub fn new() -> Self {
        Self {
            calls: Vec::with_capacity(256),
            pending: 0,
            results: [(0, 0); 8],
            result_len: 0,
            result_pos: 0,
            ack_type: event_type::ORDER_ACK,
            ack_time_offset: 0,
        }
    }

    fn record(&mut self, kind: u16, now: i64, history: Option<HistoryView<'_>>) {
        if self.calls.len() < self.calls.capacity() {
            let (len, latest) = history.map_or((0, None), |h| {
                (h.len(), h.latest().map(|e| e.header.timestamp_ns))
            });
            self.calls.push((kind, now, len, latest));
        }
    }

    fn push_result(&mut self, kind: u16, timestamp_ns: i64) {
        if let Some(slot) = self.results.get_mut(self.result_len) {
            *slot = (kind, timestamp_ns);
            self.result_len += 1;
        }
    }
}

impl ExecutionPort for FillNextExec {
    fn on_arrival(&mut self, event: EventRecord<'_>) -> Result<(), PortError> {
        let now = event.header.timestamp_ns;
        self.record(event_type::ACTION_ARRIVAL, now, None);
        self.pending += 1;
        self.push_result(self.ack_type, now + self.ack_time_offset);
        Ok(())
    }

    fn on_market_event(
        &mut self,
        event: EventRecord<'_>,
        history: HistoryView<'_>,
    ) -> Result<(), PortError> {
        let now = event.header.timestamp_ns;
        self.record(event.header.event_type, now, Some(history));
        for _ in 0..self.pending {
            self.push_result(event_type::FILL, now);
        }
        self.pending = 0;
        Ok(())
    }

    fn poll_result(
        &mut self,
        header: &mut EventHeader,
        _payload: &mut [u8],
    ) -> Result<bool, PortError> {
        if self.result_pos >= self.result_len {
            self.result_pos = 0;
            self.result_len = 0;
            return Ok(false);
        }
        let (kind, timestamp_ns) = self.results[self.result_pos];
        self.result_pos += 1;
        *header = EventHeader {
            timestamp_ns,
            event_type: kind,
            ..EventHeader::default()
        };
        Ok(true)
    }
}

/// 受け取ったイベントを記録し、指定した時刻のイベントでアクションを出す戦略。
pub struct RecordingStrategy {
    /// `(event_type, timestamp_ns, sequence_id, stream_id, 履歴の長さ, 履歴の最新の時刻)`。
    pub seen: Vec<(u16, i64, u64, u32, usize, Option<i64>)>,
    /// `(この時刻のイベントで, latency_ns)`。
    pub submit_on: Option<(i64, i64)>,
    pub submit_status: Vec<i32>,
    /// この時刻のイベントで返す `StrategyStatus`。
    pub fail_on: Option<(i64, u32)>,
}

impl RecordingStrategy {
    pub fn new() -> Self {
        Self {
            seen: Vec::with_capacity(256),
            submit_on: None,
            submit_status: Vec::with_capacity(256),
            fail_on: None,
        }
    }
}

impl Strategy for RecordingStrategy {
    fn on_event(&mut self, context: &mut Context<'_>, event: EventRecord<'_>) -> StrategyStatus {
        let h = event.header;
        let history = context.history();
        if self.seen.len() < self.seen.capacity() {
            self.seen.push((
                h.event_type,
                h.timestamp_ns,
                h.sequence_id,
                h.stream_id,
                history.len(),
                history.latest().map(|e| e.header.timestamp_ns),
            ));
        }
        if let Some((at, latency_ns)) = self.submit_on
            && at == h.timestamp_ns
            && event_type::is_market(h.event_type)
        {
            let status = context.submit_action(&Action {
                action_kind: 1,
                latency_ns,
                payload: &[9],
            });
            if self.submit_status.len() < self.submit_status.capacity() {
                self.submit_status.push(status);
            }
        }
        match self.fail_on {
            Some((at, status)) if at == h.timestamp_ns => status,
            _ => 0,
        }
    }
}

pub type TestKernel = Kernel<VecFeed, FillNextExec, RecordingStrategy>;

pub fn kernel(feed: VecFeed, strategy: RecordingStrategy) -> TestKernel {
    match Kernel::create(config(), feed, FillNextExec::new(), strategy) {
        Ok(kernel) => kernel,
        Err(code) => panic!("create が失敗した: {code}"),
    }
}

pub fn seen(kernel: &TestKernel) -> Vec<(u16, i64, u64, u32, usize, Option<i64>)> {
    kernel.inspect(|_, _, s| s.seen.clone()).unwrap()
}
