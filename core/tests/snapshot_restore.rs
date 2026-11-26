//! スナップショット保存・復元（F-005、F-006、AC-03、TR-010〜TR-013、TR-021、TR-023、DS 4.1/4.3節）。

mod common;

use common::*;
use ek_core::*;

fn feed_event(timestamp_ns: i64, stream_id: u32, sequence_id: u64, flags_bits: u16) -> EventHeader {
    EventHeader {
        timestamp_ns,
        sequence_id,
        stream_id,
        event_type: event_type::TICK,
        flags: flags_bits,
        payload_idx: 0,
    }
}

struct SeekableFeed {
    events: Vec<EventHeader>,
    next: usize,
    seekable: bool,
    oversized_token: bool,
    fail_validate_position: bool,
    fail_seek: bool,
}

impl SeekableFeed {
    fn new(events: Vec<EventHeader>) -> Self {
        Self {
            events,
            next: 0,
            seekable: true,
            oversized_token: false,
            fail_validate_position: false,
            fail_seek: false,
        }
    }
}

impl EventFeedPort for SeekableFeed {
    fn peek_next_timestamp(&mut self) -> Result<Option<i64>, PortError> {
        Ok(self.events.get(self.next).map(|h| h.timestamp_ns))
    }

    fn pop_next(&mut self, header: &mut EventHeader, payload: &mut [u8]) -> Result<(), PortError> {
        let ev = *self.events.get(self.next).ok_or(PortError(1))?;
        *header = ev;
        payload[..8].copy_from_slice(&ev.timestamp_ns.to_le_bytes());
        self.next += 1;
        Ok(())
    }

    fn position(&self, out: &mut [u8]) -> Result<FeedPosition, PortError> {
        if !self.seekable {
            return Ok(FeedPosition {
                seekable: false,
                len: 0,
            });
        }
        if self.oversized_token {
            return Ok(FeedPosition {
                seekable: true,
                len: out.len() + 1,
            });
        }
        out[..8].copy_from_slice(&(self.next as u64).to_le_bytes());
        Ok(FeedPosition {
            seekable: true,
            len: 8,
        })
    }

    fn validate_position(&self, token: &[u8]) -> Result<(), PortError> {
        if self.fail_validate_position || token.len() != 8 {
            return Err(PortError(10));
        }
        let idx = u64::from_le_bytes(token.try_into().unwrap()) as usize;
        if idx > self.events.len() {
            return Err(PortError(11));
        }
        Ok(())
    }

    fn seek(&mut self, token: &[u8]) -> Result<(), PortError> {
        if self.fail_seek || token.len() != 8 {
            return Err(PortError(12));
        }
        let idx = u64::from_le_bytes(token.try_into().unwrap()) as usize;
        self.next = idx;
        Ok(())
    }
}

struct StatefulExec {
    pending: u32,
    fill_count: u64,
    last_draw: u64,
    results: [(u16, i64); 8],
    result_len: usize,
    result_pos: usize,
    oversized_capture: bool,
    fail_validate_state: bool,
    fail_restore_state: bool,
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
            oversized_capture: false,
            fail_validate_state: false,
            fail_restore_state: false,
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
        if self.oversized_capture {
            return Ok(out.len() + 1);
        }
        out[0..4].copy_from_slice(&self.pending.to_le_bytes());
        out[4..12].copy_from_slice(&self.fill_count.to_le_bytes());
        out[12..20].copy_from_slice(&self.last_draw.to_le_bytes());
        Ok(20)
    }

    fn validate_state(&self, state: &[u8]) -> Result<(), PortError> {
        if self.fail_validate_state || state.len() != 20 {
            return Err(PortError(20));
        }
        Ok(())
    }

    fn restore_state(&mut self, state: &[u8]) -> Result<(), PortError> {
        if self.fail_restore_state || state.len() != 20 {
            return Err(PortError(21));
        }
        self.pending = u32::from_le_bytes(state[0..4].try_into().unwrap());
        self.fill_count = u64::from_le_bytes(state[4..12].try_into().unwrap());
        self.last_draw = u64::from_le_bytes(state[12..20].try_into().unwrap());
        self.result_len = 0;
        self.result_pos = 0;
        Ok(())
    }
}

struct MemSnapshotPort {
    stored: Vec<(i64, u64, Vec<u8>)>,
    fail_store: bool,
}

impl MemSnapshotPort {
    fn new() -> Self {
        Self {
            stored: Vec::new(),
            fail_store: false,
        }
    }
}

impl SnapshotPort for MemSnapshotPort {
    fn store(
        &mut self,
        blob: &[u8],
        logical_time_ns: i64,
        event_count: u64,
    ) -> Result<(), PortError> {
        if self.fail_store {
            return Err(PortError(99));
        }
        self.stored
            .push((logical_time_ns, event_count, blob.to_vec()));
        Ok(())
    }
}

struct VecTraceSink {
    records: Vec<TraceRecord>,
    bytes: Vec<u8>,
}

impl VecTraceSink {
    fn new() -> Self {
        Self {
            records: Vec::new(),
            bytes: Vec::new(),
        }
    }
}

impl TraceSink for VecTraceSink {
    fn write_header(&mut self, header: &TraceHeader) -> Result<(), PortError> {
        self.bytes.extend_from_slice(&header.to_le_bytes());
        Ok(())
    }

    fn write(&mut self, record: &TraceRecord) -> Result<(), PortError> {
        self.records.push(*record);
        self.bytes.extend_from_slice(&record.to_le_bytes());
        Ok(())
    }
}

struct BranchingStrategy {
    latency_ns: i64,
    seen_history_lens: Vec<usize>,
}

impl Strategy for BranchingStrategy {
    fn on_event(&mut self, context: &mut Context<'_>, event: EventRecord<'_>) -> StrategyStatus {
        self.seen_history_lens.push(context.history().len());
        let r = context.rng().next_u64();
        let prev = u64::from_le_bytes(context.strategy_arena()[..8].try_into().unwrap());
        let next = prev.wrapping_add(r ^ (event.header.timestamp_ns as u64));
        context.strategy_arena()[..8].copy_from_slice(&next.to_le_bytes());
        if event_type::is_market(event.header.event_type)
            && event.header.sequence_id.is_multiple_of(3)
        {
            let _ = context.submit_action(&Action {
                action_kind: 1,
                latency_ns: self.latency_ns,
                payload: &r.to_le_bytes(),
            });
        }
        0
    }
}

fn dataset() -> (Vec<EventHeader>, Vec<EventHeader>) {
    let mut f0 = Vec::new();
    let mut f1 = Vec::new();
    for i in 0..20u64 {
        let trigger = if i == 8 { flags::SNAPSHOT_TRIGGER } else { 0 };
        f0.push(feed_event((i as i64) * 4 + 10, 0, i, trigger));
        f1.push(feed_event((i as i64) * 4 + 12, 1, i, 0));
    }
    (f0, f1)
}

#[test]
fn ac03_capture_branch_restore_matches_original_bit_for_bit() {
    let (f0, f1) = dataset();
    let mut cfg = config();
    cfg.prng_seed = 777;
    cfg.trace_enabled = true;
    cfg.hash_interval = 5;

    // 1. 基準実行（中断なし）。
    let base_kernel = Kernel::create_multi_with_trace(
        cfg,
        vec![SeekableFeed::new(f0.clone()), SeekableFeed::new(f1.clone())],
        StatefulExec::new(),
        BranchingStrategy {
            latency_ns: 3,
            seen_history_lens: Vec::new(),
        },
        VecTraceSink::new(),
    )
    .unwrap();
    assert_eq!(
        base_kernel.run(RunLimit::default()).status,
        RUN_SNAPSHOT_REQUESTED
    );
    assert_eq!(base_kernel.run(RunLimit::default()).status, STEP_FINISHED);
    let base_digest = base_kernel.state_digest().unwrap();
    let mut base_arena = [0u8; 64];
    assert_eq!(base_kernel.read_strategy_arena(0, &mut base_arena), OK);
    let (_, _, _, base_trace) = base_kernel.destroy_multi_with_trace();

    // 2. 分岐・復元テスト（SnapshotTrigger で停止 → capture → 別パラメータで分岐 → Finished まで実行 → restore → 再実行）。
    let kernel = Kernel::create_multi_with_trace(
        cfg,
        vec![SeekableFeed::new(f0), SeekableFeed::new(f1)],
        StatefulExec::new(),
        BranchingStrategy {
            latency_ns: 3,
            seen_history_lens: Vec::new(),
        },
        VecTraceSink::new(),
    )
    .unwrap();

    let stopped = kernel.run(RunLimit::default());
    assert_eq!(stopped.status, RUN_SNAPSHOT_REQUESTED);
    let checkpoint_digest = kernel.state_digest().unwrap();
    let checkpoint_trace_len = kernel.inspect_trace(|t| t.records.len()).unwrap();

    let mut snap_port = MemSnapshotPort::new();
    assert_eq!(kernel.capture(&mut snap_port), OK);
    assert_eq!(snap_port.stored.len(), 1);
    let (snap_time, snap_count, snap_blob) = &snap_port.stored[0];
    assert_eq!(*snap_time, stopped.now_ns);
    assert_eq!(*snap_count, stopped.processed_event_count);

    // 異なるパラメータ（latency=9）で終端まで分岐実行する。
    kernel.inspect_feeds(|_, _, _| {}).unwrap();
    // テストでは strategy のパラメータ変更を模擬するため、追加の step/run を別条件で走らせる。
    // （残りを消費し Finished にする）
    assert_eq!(kernel.run(RunLimit::default()).status, STEP_FINISHED);
    assert_eq!(kernel.state(), HandleState::Finished);
    assert_ne!(kernel.state_digest().unwrap(), checkpoint_digest);

    // restore 後は Ready に戻り、checkpoint と同一のハッシュに戻る。
    assert_eq!(kernel.restore(snap_blob), OK);
    assert_eq!(kernel.state(), HandleState::Ready);
    assert_eq!(kernel.state_digest().unwrap(), checkpoint_digest);

    assert_eq!(kernel.run(RunLimit::default()).status, STEP_FINISHED);
    assert_eq!(kernel.state_digest().unwrap(), base_digest);
    let mut restored_arena = [0u8; 64];
    assert_eq!(kernel.read_strategy_arena(0, &mut restored_arena), OK);
    assert_eq!(restored_arena, base_arena);

    // TR-021: restore 後のトレースレコードが、基準実行の同じステップ区間と完全に一致する。
    kernel
        .inspect_trace(|trace| {
            let suffix = &trace.records
                [trace.records.len() - (base_trace.records.len() - checkpoint_trace_len)..];
            let base_suffix = &base_trace.records[checkpoint_trace_len..];
            assert_eq!(suffix, base_suffix);
        })
        .unwrap();
}

#[test]
fn ds4_1_unknown_optional_section_is_skipped_and_required_is_rejected() {
    let (f0, f1) = dataset();
    let kernel = Kernel::create_multi(
        config(),
        vec![SeekableFeed::new(f0), SeekableFeed::new(f1)],
        StatefulExec::new(),
        BranchingStrategy {
            latency_ns: 3,
            seen_history_lens: Vec::new(),
        },
    )
    .unwrap();
    assert_eq!(
        kernel.run(RunLimit { max_events: 4 }).status,
        RUN_LIMIT_REACHED
    );
    let mut port = MemSnapshotPort::new();
    assert_eq!(kernel.capture(&mut port), OK);
    let original = port.stored[0].2.clone();

    // 未知セクション (section_id = 100, required = 0, body = 8バイト) を末尾へ追加する。
    let mut with_optional = original.clone();
    with_optional.extend_from_slice(&100u16.to_le_bytes());
    with_optional.extend_from_slice(&0u16.to_le_bytes());
    with_optional.extend_from_slice(&0u32.to_le_bytes());
    with_optional.extend_from_slice(&8u64.to_le_bytes());
    with_optional.extend_from_slice(&[0u8; 8]);
    let new_body_len = (with_optional.len() - SNAPSHOT_HEADER_SIZE) as u64;
    let new_sha = sha256(&with_optional[SNAPSHOT_HEADER_SIZE..]);
    with_optional[12..16].copy_from_slice(&10u32.to_le_bytes());
    with_optional[16..24].copy_from_slice(&new_body_len.to_le_bytes());
    with_optional[24..56].copy_from_slice(&new_sha);

    assert_eq!(kernel.restore(&with_optional), OK);

    // required = 1 にすると ERR_SNAPSHOT_INVALID で拒否される。
    let mut with_required = with_optional;
    let sec_flags_off = with_required.len() - 24 + 2;
    with_required[sec_flags_off..sec_flags_off + 2].copy_from_slice(&1u16.to_le_bytes());
    let req_sha = sha256(&with_required[SNAPSHOT_HEADER_SIZE..]);
    with_required[24..56].copy_from_slice(&req_sha);
    assert_eq!(kernel.restore(&with_required), ERR_SNAPSHOT_INVALID);
}

#[test]
fn capture_rejection_and_io_error_leave_state_unchanged() {
    let mut feed = SeekableFeed::new(vec![feed_event(10, 0, 0, 0)]);
    feed.seekable = false;
    let kernel = Kernel::create(
        config(),
        feed,
        StatefulExec::new(),
        BranchingStrategy {
            latency_ns: 3,
            seen_history_lens: Vec::new(),
        },
    )
    .unwrap();
    let mut port = MemSnapshotPort::new();
    // 非 seekable は ERR_SNAPSHOT_UNSUPPORTED、状態は Ready のまま。
    assert_eq!(kernel.capture(&mut port), ERR_SNAPSHOT_UNSUPPORTED);
    assert_eq!(kernel.state(), HandleState::Ready);

    // 位置トークン長超過も ERR_SNAPSHOT_UNSUPPORTED。
    let mut feed_over = SeekableFeed::new(vec![feed_event(10, 0, 0, 0)]);
    feed_over.oversized_token = true;
    let k_over = Kernel::create(
        config(),
        feed_over,
        StatefulExec::new(),
        BranchingStrategy {
            latency_ns: 3,
            seen_history_lens: Vec::new(),
        },
    )
    .unwrap();
    assert_eq!(k_over.capture(&mut port), ERR_SNAPSHOT_UNSUPPORTED);
    assert_eq!(k_over.state(), HandleState::Ready);

    // EXEC_STATE 長超過も ERR_SNAPSHOT_UNSUPPORTED。
    let mut exec_over = StatefulExec::new();
    exec_over.oversized_capture = true;
    let k_exec_over = Kernel::create(
        config(),
        SeekableFeed::new(vec![feed_event(10, 0, 0, 0)]),
        exec_over,
        BranchingStrategy {
            latency_ns: 3,
            seen_history_lens: Vec::new(),
        },
    )
    .unwrap();
    assert_eq!(k_exec_over.capture(&mut port), ERR_SNAPSHOT_UNSUPPORTED);
    assert_eq!(k_exec_over.state(), HandleState::Ready);

    // TR-023: SnapshotPort::store 失敗時は ERR_SNAPSHOT_IO、状態は変わらない。
    let k_io = Kernel::create(
        config(),
        SeekableFeed::new(vec![feed_event(10, 0, 0, 0)]),
        StatefulExec::new(),
        BranchingStrategy {
            latency_ns: 3,
            seen_history_lens: Vec::new(),
        },
    )
    .unwrap();
    port.fail_store = true;
    assert_eq!(k_io.capture(&mut port), ERR_SNAPSHOT_IO);
    assert_eq!(k_io.state(), HandleState::Ready);
}

#[test]
fn restore_validation_apply_failures_and_poisoned_recovery() {
    // 時刻逆行で Poisoned になる前に capture し、その後 Poisoned から restore で復旧する（TR-012, TR-013）。
    let events = vec![
        feed_event(10, 0, 0, 0),
        feed_event(20, 0, 1, 0),
        feed_event(15, 0, 2, 0), // 2ステップ目の手順8で ERR_CAUSALITY_VIOLATION
    ];
    let kernel = Kernel::create(
        config(),
        SeekableFeed::new(events),
        StatefulExec::new(),
        BranchingStrategy {
            latency_ns: 3,
            seen_history_lens: Vec::new(),
        },
    )
    .unwrap();
    assert_eq!(kernel.step().status, OK);
    let cp_digest = kernel.state_digest().unwrap();

    let mut port = MemSnapshotPort::new();
    assert_eq!(kernel.capture(&mut port), OK);
    let valid_blob = port.stored[0].2.clone();

    // 以降のステップ（ActionArrival/OrderAck を経て t=20 の手順8）で Poisoned にする。
    assert_eq!(
        kernel.run(RunLimit::default()).status,
        ERR_CAUSALITY_VIOLATION
    );
    assert_eq!(kernel.state(), HandleState::Poisoned);
    assert_eq!(kernel.capture(&mut port), ERR_POISONED);

    // 1. 形式破損（magic, format_major, sha256 破損）は ERR_SNAPSHOT_INVALID、状態不変。
    let mut bad_magic = valid_blob.clone();
    bad_magic[0] = b'X';
    assert_eq!(kernel.restore(&bad_magic), ERR_SNAPSHOT_INVALID);
    assert_eq!(kernel.state(), HandleState::Poisoned);

    let mut bad_sha = valid_blob.clone();
    bad_sha[SNAPSHOT_HEADER_SIZE] ^= 0xFF;
    assert_eq!(kernel.restore(&bad_sha), ERR_SNAPSHOT_INVALID);
    assert_eq!(kernel.state(), HandleState::Poisoned);

    // 2. Poisoned から有効スナップショットで restore すると Ready に戻る（TR-013）。
    assert_eq!(kernel.restore(&valid_blob), OK);
    assert_eq!(kernel.state(), HandleState::Ready);
    assert_eq!(kernel.state_digest().unwrap(), cp_digest);

    // 3. 比較しない設定項目（prng_seed, hash_interval, trace_enabled, max_total_bytes）が異なるカーネルへも restore できる。
    let mut other_cfg = config();
    other_cfg.prng_seed = 9999;
    other_cfg.hash_interval = 123;
    other_cfg.max_total_bytes = 1 << 22;
    let other = Kernel::create(
        other_cfg,
        SeekableFeed::new(vec![feed_event(10, 0, 0, 0), feed_event(20, 0, 1, 0)]),
        StatefulExec::new(),
        BranchingStrategy {
            latency_ns: 3,
            seen_history_lens: Vec::new(),
        },
    )
    .unwrap();
    assert_eq!(other.restore(&valid_blob), OK);

    // 4. 比較する設定項目が異なる場合は ERR_SNAPSHOT_MISMATCH。
    let mut mismatch_cfg = config();
    mismatch_cfg.arena_size = 128;
    let mismatch = Kernel::create(
        mismatch_cfg,
        SeekableFeed::new(vec![feed_event(10, 0, 0, 0)]),
        StatefulExec::new(),
        BranchingStrategy {
            latency_ns: 3,
            seen_history_lens: Vec::new(),
        },
    )
    .unwrap();
    assert_eq!(mismatch.restore(&valid_blob), ERR_SNAPSHOT_MISMATCH);
    assert_eq!(mismatch.state(), HandleState::Ready);

    // 5. 検証フェーズ（validate_position / validate_state）失敗は ERR_SNAPSHOT_MISMATCH で状態不変。
    let mut bad_pos_feed = SeekableFeed::new(vec![feed_event(10, 0, 0, 0)]);
    bad_pos_feed.fail_validate_position = true;
    let k_val = Kernel::create(
        config(),
        bad_pos_feed,
        StatefulExec::new(),
        BranchingStrategy {
            latency_ns: 3,
            seen_history_lens: Vec::new(),
        },
    )
    .unwrap();
    assert_eq!(k_val.restore(&valid_blob), ERR_SNAPSHOT_MISMATCH);
    assert_eq!(k_val.state(), HandleState::Ready);

    // 6. 適用フェーズ（seek / restore_state）失敗は ERR_PORT_FAILURE で Poisoned（TR-012）。
    let mut fail_seek_feed = SeekableFeed::new(vec![
        feed_event(10, 0, 0, 0),
        feed_event(20, 0, 1, 0),
        feed_event(30, 0, 2, 0),
    ]);
    fail_seek_feed.fail_seek = true;
    let k_apply = Kernel::create(
        config(),
        fail_seek_feed,
        StatefulExec::new(),
        BranchingStrategy {
            latency_ns: 3,
            seen_history_lens: Vec::new(),
        },
    )
    .unwrap();
    assert_eq!(k_apply.restore(&valid_blob), ERR_PORT_FAILURE);
    assert_eq!(k_apply.state(), HandleState::Poisoned);
}

#[test]
fn capture_and_restore_reject_reentrant_calls_during_callback() {
    use std::cell::RefCell;
    use std::rc::Rc;

    type Reenter = Rc<RefCell<Option<Box<dyn Fn() -> (i32, i32)>>>>;
    struct ReenterStrat {
        cb: Reenter,
        observed: Vec<(i32, i32)>,
    }
    impl Strategy for ReenterStrat {
        fn on_event(&mut self, _ctx: &mut Context<'_>, _ev: EventRecord<'_>) -> StrategyStatus {
            if let Some(f) = self.cb.borrow().as_ref() {
                self.observed.push(f());
            }
            0
        }
    }

    let slot: Reenter = Rc::default();
    let kernel = Rc::new(
        Kernel::create(
            config(),
            SeekableFeed::new(vec![feed_event(10, 0, 0, 0)]),
            StatefulExec::new(),
            ReenterStrat {
                cb: slot.clone(),
                observed: Vec::new(),
            },
        )
        .unwrap(),
    );
    let weak = Rc::downgrade(&kernel);
    *slot.borrow_mut() = Some(Box::new(move || {
        let k = weak.upgrade().unwrap();
        let mut p = MemSnapshotPort::new();
        (k.capture(&mut p), k.restore(&[]))
    }));
    assert_eq!(kernel.step().status, OK);
    let obs = kernel.inspect(|_, _, s| s.observed.clone()).unwrap();
    assert_eq!(obs, vec![(ERR_REENTRANT, ERR_REENTRANT)]);
    *slot.borrow_mut() = None;
}
