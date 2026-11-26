//! 状態ハッシュ・トレース（TR-016、TR-021）および決定論性（H3、AC-04、AC-07、QG-006）。

mod common;

use std::sync::Arc;
use std::thread;

use common::*;
use ek_core::*;

struct VecTraceSink {
    headers: Vec<TraceHeader>,
    records: Vec<TraceRecord>,
    bytes: Vec<u8>,
    fail_header: bool,
    fail_at_record: Option<usize>,
}

impl VecTraceSink {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            headers: Vec::with_capacity(1),
            records: Vec::with_capacity(capacity),
            bytes: Vec::with_capacity(TraceHeader::SIZE + capacity * TraceRecord::SIZE),
            fail_header: false,
            fail_at_record: None,
        }
    }

    fn log_sha256(&self) -> [u8; 32] {
        sha256(&self.bytes)
    }
}

impl TraceSink for VecTraceSink {
    fn write_header(&mut self, header: &TraceHeader) -> Result<(), PortError> {
        if self.fail_header {
            return Err(PortError(31));
        }
        self.headers.push(*header);
        self.bytes.extend_from_slice(&header.to_le_bytes());
        Ok(())
    }

    fn write(&mut self, record: &TraceRecord) -> Result<(), PortError> {
        if self.fail_at_record == Some(self.records.len()) {
            return Err(PortError(32));
        }
        self.records.push(*record);
        self.bytes.extend_from_slice(&record.to_le_bytes());
        Ok(())
    }
}

/// 不変の配列を共有して、`FEED_STATES` の位置トークンを返す決定論フィード。
struct SharedSliceFeed<'a> {
    events: &'a [EventHeader],
    payloads: &'a [u8],
    record_size: usize,
    next: usize,
    seekable: bool,
    fail_position: bool,
    oversized_position: bool,
}

impl<'a> SharedSliceFeed<'a> {
    fn new(events: &'a [EventHeader], payloads: &'a [u8], record_size: usize) -> Self {
        Self {
            events,
            payloads,
            record_size,
            next: 0,
            seekable: true,
            fail_position: false,
            oversized_position: false,
        }
    }
}

impl EventFeedPort for SharedSliceFeed<'_> {
    fn peek_next_timestamp(&mut self) -> Result<Option<i64>, PortError> {
        Ok(self.events.get(self.next).map(|h| h.timestamp_ns))
    }

    fn pop_next(&mut self, header: &mut EventHeader, payload: &mut [u8]) -> Result<(), PortError> {
        let h = *self.events.get(self.next).ok_or(PortError(1))?;
        let off = self.next * self.record_size;
        let src = self
            .payloads
            .get(off..off + self.record_size)
            .ok_or(PortError(2))?;
        *header = h;
        payload.copy_from_slice(src);
        self.next += 1;
        Ok(())
    }

    fn position(&self, out: &mut [u8]) -> Result<FeedPosition, PortError> {
        if self.fail_position {
            return Err(PortError(41));
        }
        if self.oversized_position {
            return Ok(FeedPosition {
                seekable: true,
                len: out.len() + 1,
            });
        }
        if !self.seekable {
            // seekable = false のとき、非ゼロを返しても core 側が token_len=0 に正規化する。
            out[0] = 0xAA;
            return Ok(FeedPosition {
                seekable: false,
                len: 1,
            });
        }
        let bytes = (self.next as u64).to_le_bytes();
        out[..8].copy_from_slice(&bytes);
        Ok(FeedPosition {
            seekable: true,
            len: 8,
        })
    }
}

/// PRNG を引き、決定的な内部状態（`capture_state`）を持つ約定モデル。
struct StatefulExec {
    pending: u32,
    fill_count: u64,
    last_draw: u64,
    results: [(u16, i64); 8],
    result_len: usize,
    result_pos: usize,
    fail_capture: bool,
    oversized_capture: bool,
}

impl StatefulExec {
    fn new() -> Self {
        Self {
            pending: 0,
            fill_count: 0,
            last_draw: 0,
            results: [(0, 0); 8],
            result_len: 0,
            result_pos: 0,
            fail_capture: false,
            oversized_capture: false,
        }
    }

    fn push_result(&mut self, kind: u16, ts: i64) {
        if let Some(slot) = self.results.get_mut(self.result_len) {
            *slot = (kind, ts);
            self.result_len += 1;
        }
    }
}

impl ExecutionPort for StatefulExec {
    fn on_arrival(&mut self, event: EventRecord<'_>, rng: &mut Prng) -> Result<(), PortError> {
        self.pending += 1;
        self.last_draw = rng.next_u64();
        self.push_result(event_type::ORDER_ACK, event.header.timestamp_ns);
        Ok(())
    }

    fn on_market_event(
        &mut self,
        event: EventRecord<'_>,
        _history: HistoryView<'_>,
        rng: &mut Prng,
    ) -> Result<(), PortError> {
        if self.pending > 0 {
            let _slippage = rng.next_u64();
            for _ in 0..self.pending {
                self.push_result(event_type::FILL, event.header.timestamp_ns);
                self.fill_count += 1;
            }
            self.pending = 0;
        }
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
        let (kind, ts) = self.results[self.result_pos];
        self.result_pos += 1;
        *header = EventHeader {
            timestamp_ns: ts,
            event_type: kind,
            ..EventHeader::default()
        };
        Ok(true)
    }

    fn capture_state(&self, out: &mut [u8]) -> Result<usize, PortError> {
        if self.fail_capture {
            return Err(PortError(51));
        }
        if self.oversized_capture {
            return Ok(out.len() + 1);
        }
        out[0..4].copy_from_slice(&self.pending.to_le_bytes());
        out[4..12].copy_from_slice(&self.fill_count.to_le_bytes());
        out[12..20].copy_from_slice(&self.last_draw.to_le_bytes());
        Ok(20)
    }
}

struct DeterministicStrategy {
    count: u64,
}

impl Strategy for DeterministicStrategy {
    fn on_event(&mut self, context: &mut Context<'_>, event: EventRecord<'_>) -> StrategyStatus {
        self.count += 1;
        let r = context.rng().next_u64();
        let prev = u64::from_le_bytes(context.strategy_arena()[..8].try_into().unwrap());
        let next = prev.wrapping_add(r ^ (event.header.timestamp_ns as u64));
        context.strategy_arena()[..8].copy_from_slice(&next.to_le_bytes());
        if event_type::is_market(event.header.event_type) && self.count.is_multiple_of(5) {
            let _ = context.submit_action(&Action {
                action_kind: 1,
                latency_ns: 3,
                payload: &r.to_le_bytes(),
            });
        }
        0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RunOutcome {
    processed: u64,
    now_ns: i64,
    state_digest: [u8; 32],
    trace_sha256: [u8; 32],
    arena: [u8; 64],
}

fn build_dataset(count: usize) -> (Vec<EventHeader>, Vec<u8>) {
    let mut events = Vec::with_capacity(count);
    let mut payloads = vec![0u8; count * PAYLOAD as usize];
    for i in 0..count {
        events.push(EventHeader {
            timestamp_ns: (i as i64) * 2 + 100,
            sequence_id: i as u64,
            stream_id: 0,
            event_type: event_type::TICK,
            flags: 0,
            // フィードが非ゼロの payload_idx を返しても、正準バイト列・ハッシュは 0 扱いになる。
            payload_idx: (i as u64) + 777,
        });
        let p = (10_000 + (i as i64 * 17) % 500).to_le_bytes();
        payloads[i * PAYLOAD as usize..i * PAYLOAD as usize + 8].copy_from_slice(&p);
    }
    (events, payloads)
}

fn run_scenario(events: &[EventHeader], payloads: &[u8], seed: u64) -> RunOutcome {
    let mut cfg = config();
    cfg.prng_seed = seed;
    cfg.hash_interval = 10;
    cfg.trace_enabled = true;

    let kernel = Kernel::create_with_trace(
        cfg,
        SharedSliceFeed::new(events, payloads, PAYLOAD as usize),
        StatefulExec::new(),
        DeterministicStrategy { count: 0 },
        VecTraceSink::with_capacity(events.len() * 4),
    )
    .unwrap();
    let init = [0x11u8; 16];
    assert_eq!(kernel.init_strategy_arena(0, &init), OK);

    let res = kernel.run(RunLimit::default());
    assert_eq!(res.status, STEP_FINISHED);

    let state_digest = kernel.state_digest().unwrap();
    let mut arena = [0u8; 64];
    assert_eq!(kernel.read_strategy_arena(0, &mut arena), OK);
    let (_, _, _, trace) = kernel.destroy_with_trace();
    let trace_sha256 = trace.log_sha256();
    RunOutcome {
        processed: res.processed_event_count,
        now_ns: res.now_ns,
        state_digest,
        trace_sha256,
        arena,
    }
}

#[test]
fn qg006_single_and_16_parallel_runs_produce_identical_hashes() {
    let (events, payloads) = build_dataset(500);
    let events = Arc::new(events);
    let payloads = Arc::new(payloads);

    let baseline = run_scenario(&events, &payloads, 42);
    let different_seed = run_scenario(&events, &payloads, 43);
    assert_ne!(baseline.state_digest, different_seed.state_digest);
    assert_ne!(baseline.trace_sha256, different_seed.trace_sha256);

    let outcomes: Vec<RunOutcome> = thread::scope(|scope| {
        let handles: Vec<_> = (0..16)
            .map(|_| {
                let events = Arc::clone(&events);
                let payloads = Arc::clone(&payloads);
                scope.spawn(move || run_scenario(&events, &payloads, 42))
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });

    for (idx, outcome) in outcomes.iter().enumerate() {
        assert_eq!(outcome, &baseline, "parallel worker {idx} diverged");
    }
}

#[test]
fn tr021_step_hash_matches_canonical_bytes_and_interval() {
    let (events, payloads) = build_dataset(10);
    let mut cfg = config();
    cfg.hash_interval = 3;
    cfg.trace_enabled = true;

    let kernel = Kernel::create_with_trace(
        cfg,
        SharedSliceFeed::new(&events, &payloads, PAYLOAD as usize),
        StatefulExec::new(),
        DeterministicStrategy { count: 0 },
        VecTraceSink::with_capacity(32),
    )
    .unwrap();

    let mut canonical_step = vec![0u8; 16384];
    let mut canonical_full = vec![0u8; 16384];

    for step_idx in 1..=6u64 {
        assert_eq!(kernel.step().status, OK);
        let len_step = kernel
            .write_canonical_state(false, &mut canonical_step)
            .unwrap();
        let expected_xxh3 = xxh3_64(&canonical_step[..len_step]);

        let len_full = kernel
            .write_canonical_state(true, &mut canonical_full)
            .unwrap();
        assert_eq!(
            kernel.state_digest().unwrap(),
            sha256(&canonical_full[..len_full])
        );

        kernel
            .inspect_trace(|trace| {
                assert_eq!(trace.records.len(), step_idx as usize);
                let rec = trace.records.last().unwrap();
                if step_idx % 3 == 0 {
                    assert_eq!(rec.trace_flags, TRACE_FLAG_HASH_PRESENT);
                    assert_eq!(rec.state_hash, expected_xxh3);
                } else {
                    assert_eq!(rec.trace_flags, 0);
                    assert_eq!(rec.state_hash, 0);
                }
            })
            .unwrap();
    }
}

#[test]
fn trace_sink_validation_and_error_handling() {
    let (events, payloads) = build_dataset(4);
    let mut cfg = config();
    cfg.trace_enabled = true;

    // 1. `None::<VecTraceSink>` で trace_enabled=true は ERR_INVALID_ARGUMENT。
    assert_eq!(
        Kernel::create_with_trace(
            cfg,
            SharedSliceFeed::new(&events, &payloads, PAYLOAD as usize),
            StatefulExec::new(),
            DeterministicStrategy { count: 0 },
            None::<VecTraceSink>,
        )
        .err(),
        Some(ERR_INVALID_ARGUMENT)
    );

    // 2. write_header 失敗時は ERR_PORT_FAILURE でハンドルを作らない。
    let mut bad_header_sink = VecTraceSink::with_capacity(8);
    bad_header_sink.fail_header = true;
    assert_eq!(
        Kernel::create_with_trace(
            cfg,
            SharedSliceFeed::new(&events, &payloads, PAYLOAD as usize),
            StatefulExec::new(),
            DeterministicStrategy { count: 0 },
            bad_header_sink,
        )
        .err(),
        Some(ERR_PORT_FAILURE)
    );

    // 3. trace_enabled=false のとき、TraceSink があっても何も書かない。
    let mut disabled_cfg = config();
    disabled_cfg.trace_enabled = false;
    let kernel = Kernel::create_with_trace(
        disabled_cfg,
        SharedSliceFeed::new(&events, &payloads, PAYLOAD as usize),
        StatefulExec::new(),
        DeterministicStrategy { count: 0 },
        Some(VecTraceSink::with_capacity(8)),
    )
    .unwrap();
    assert_eq!(kernel.run(RunLimit::default()).status, STEP_FINISHED);
    kernel
        .inspect_trace(|t| {
            let t = t.as_ref().unwrap();
            assert!(t.headers.is_empty());
            assert!(t.records.is_empty());
        })
        .unwrap();

    // 4. トレース書き込み失敗時は ERR_PORT_FAILURE で Poisoned に遷移する。
    let mut fail_write_sink = VecTraceSink::with_capacity(8);
    fail_write_sink.fail_at_record = Some(1);
    let kernel = Kernel::create_with_trace(
        cfg,
        SharedSliceFeed::new(&events, &payloads, PAYLOAD as usize),
        StatefulExec::new(),
        DeterministicStrategy { count: 0 },
        fail_write_sink,
    )
    .unwrap();
    assert_eq!(kernel.step().status, OK);
    let second = kernel.step();
    assert_eq!(second.status, ERR_PORT_FAILURE);
    assert_eq!(second.detail, 32);
    assert_eq!(kernel.state(), HandleState::Poisoned);
    assert_eq!(kernel.state_digest(), Err(ERR_POISONED));
}

#[test]
fn failed_refill_step_does_not_emit_trace_and_non_seekable_digest_succeeds() {
    // 時刻逆行: 2件目のステップではなく 1件目の手順8 で ERR_CAUSALITY_VIOLATION が返り、
    // 違反ステップのトレースは出力されない（EK-DD-001 3.1節・TR-004）。
    let mut cfg = config();
    cfg.trace_enabled = true;
    cfg.hash_interval = 1;
    let kernel = Kernel::create_with_trace(
        cfg,
        VecFeed::ticks(&[100, 90]),
        FillNextExec::new(),
        RecordingStrategy::new(),
        VecTraceSink::with_capacity(8),
    )
    .unwrap();
    let res = kernel.step();
    assert_eq!(res.status, ERR_CAUSALITY_VIOLATION);
    assert_eq!(kernel.state(), HandleState::Poisoned);
    kernel
        .inspect_trace(|t| {
            assert_eq!(t.headers.len(), 1);
            assert!(t.records.is_empty());
        })
        .unwrap();

    // seekable=false のフィードでも state_digest() とステップハッシュは成功する。
    let (events, payloads) = build_dataset(3);
    let mut feed_a = SharedSliceFeed::new(&events, &payloads, PAYLOAD as usize);
    feed_a.seekable = false;
    let mut feed_b = SharedSliceFeed::new(&events, &payloads, PAYLOAD as usize);
    feed_b.seekable = false;

    let ka = Kernel::create_with_trace(
        cfg,
        feed_a,
        StatefulExec::new(),
        DeterministicStrategy { count: 0 },
        VecTraceSink::with_capacity(16),
    )
    .unwrap();
    let kb = Kernel::create_with_trace(
        cfg,
        feed_b,
        StatefulExec::new(),
        DeterministicStrategy { count: 0 },
        VecTraceSink::with_capacity(16),
    )
    .unwrap();
    assert_eq!(ka.run(RunLimit::default()).status, STEP_FINISHED);
    assert_eq!(kb.run(RunLimit::default()).status, STEP_FINISHED);
    assert_eq!(ka.state_digest().unwrap(), kb.state_digest().unwrap());

    // ポート状態サイズ超過・取得失敗時は ERR_PORT_FAILURE になり状態を変えない。
    let mut oversized_exec = StatefulExec::new();
    oversized_exec.oversized_capture = true;
    let kc = Kernel::create(
        config(),
        SharedSliceFeed::new(&events, &payloads, PAYLOAD as usize),
        oversized_exec,
        DeterministicStrategy { count: 0 },
    )
    .unwrap();
    assert_eq!(kc.state_digest(), Err(ERR_PORT_FAILURE));
    assert_eq!(kc.state(), HandleState::Ready);
}
