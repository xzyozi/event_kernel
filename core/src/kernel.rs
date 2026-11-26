//! カーネルのハンドルと、ステップの手順（EK-DD-001 2.2節、3.1節、3.2節）。

use alloc::vec;
use alloc::vec::Vec;
use core::cell::RefCell;

use crate::canonical::{CanonicalInputs, CanonicalScratch, write_canonical_sections};
use crate::config::KernelConfig;
use crate::event::{
    ACTION_PAYLOAD_OFFSET, ACTION_STREAM_ID, EventHeader, EventRecord, event_type, flags,
};
use crate::hash::{sha256, xxh3_64};
use crate::history::{HistoryRing, HistoryView};
use crate::ports::{EventFeedPort, ExecutionPort, NoTraceSink, PortError, Strategy, TraceSink};
use crate::prng::Prng;
use crate::queue::ActionQueue;
use crate::status::*;
use crate::trace::{TRACE_FLAG_HASH_PRESENT, TraceHeader, TraceRecord};
use crate::util::copy_bytes;

/// 最初のイベントを処理するまでの論理時刻（`i64` の最小値）。
pub const CLOCK_UNSET_NS: i64 = i64::MIN;

/// 外部フィードの `stream_id`。縦切りでは、外部フィードは1本だけである。
const FEED_STREAM_ID: u32 = 0;

/// ハンドルの状態（EK-DD-001 3.2節）。`Running` は、ステップの実行中に外から観測したときだけ返る。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandleState {
    /// 操作を受け付ける。
    Ready,
    /// ステップの実行中（戦略のコールバック中）。
    Running,
    /// 入力が尽きた。
    Finished,
    /// 致命的エラーの後。
    Poisoned,
}

/// `step` と `run` の結果（EK-DD-001 2.4節）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StepResult {
    /// 終了コード。
    pub status: i32,
    /// 戦略またはポートが返した詳細コード。ない場合は0。
    pub detail: i32,
    /// 処理済みイベントの累計。
    pub processed_event_count: u64,
    /// 現在の論理時刻。
    pub now_ns: i64,
}

/// `run` の停止条件（EK-DD-001 2.3節 F-003）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RunLimit {
    /// 処理するイベント数の上限。0は無制限。
    pub max_events: u64,
}

/// 戦略が提出するアクション（EK-DD-001 2.3節 F-004）。
#[derive(Clone, Copy, Debug)]
pub struct Action<'a> {
    /// 戦略と `ExecutionPort` の間で決める値。`core` は解釈しない。
    pub action_kind: u16,
    /// 到達までの遅延。1以上。
    pub latency_ns: i64,
    /// `ExecutionPort` が解釈するバイト列。`payload_record_size - 8` バイト以下。不足分は0で埋める。
    pub payload: &'a [u8],
}

/// 戦略へ渡す `Context`（EK-DD-001 2.3節 F-010）。コールバック中だけ有効。
pub struct Context<'a> {
    now_ns: i64,
    history: HistoryView<'a>,
    strategy_arena: &'a mut [u8],
    rng: &'a mut Prng,
    queue: &'a mut ActionQueue,
    next_action_sequence_id: &'a mut u64,
}

impl Context<'_> {
    /// 現在の論理時刻。
    pub fn now_ns(&self) -> i64 {
        self.now_ns
    }

    /// 履歴ウィンドウ。市場イベントの通知では、今のイベントを含む。
    pub fn history(&self) -> HistoryView<'_> {
        self.history
    }

    /// `StrategyArena` の書き込み可能ビュー。
    pub fn strategy_arena(&mut self) -> &mut [u8] {
        self.strategy_arena
    }

    /// 決定論 PRNG。戦略は、これ以外の乱数源を使わない（EK-DD-001 2.6節）。
    pub fn rng(&mut self) -> &mut Prng {
        self.rng
    }

    /// アクションを提出する（F-004）。失敗しても、状態を変更しない。
    pub fn submit_action(&mut self, action: &Action<'_>) -> i32 {
        if action.latency_ns < 1 {
            return ERR_INVALID_ARGUMENT;
        }
        let Some(timestamp_ns) = self.now_ns.checked_add(action.latency_ns) else {
            return ERR_INVALID_ARGUMENT;
        };
        let capacity = self
            .queue
            .record_size()
            .saturating_sub(ACTION_PAYLOAD_OFFSET);
        if action.payload.len() > capacity {
            return ERR_INVALID_ARGUMENT;
        }
        if self.queue.is_full() {
            return ERR_QUEUE_FULL;
        }
        let header = EventHeader {
            timestamp_ns,
            sequence_id: *self.next_action_sequence_id,
            stream_id: ACTION_STREAM_ID,
            event_type: event_type::ACTION_ARRIVAL,
            flags: 0,
            payload_idx: 0,
        };
        let kind = action.action_kind.to_le_bytes();
        let pushed = self.queue.push_with(header, |payload| {
            payload.fill(0);
            if let Some(head) = payload.get_mut(..kind.len()) {
                head.copy_from_slice(&kind);
            }
            if let Some(body) = payload.get_mut(ACTION_PAYLOAD_OFFSET..) {
                copy_bytes(body, action.payload);
            }
        });
        if !pushed {
            return ERR_QUEUE_FULL;
        }
        *self.next_action_sequence_id += 1;
        OK
    }
}

/// カーネルのハンドル（BD の C-009）。本体を `RefCell` で包み、再入を検出する（EK-DD-001 3.2節）。
pub struct Kernel<F, E, S, T = NoTraceSink> {
    inner: RefCell<Inner<F, E, S, T>>,
}

struct Inner<F, E, S, T> {
    config: KernelConfig,
    state: HandleState,
    now_ns: i64,
    processed_event_count: u64,
    next_action_sequence_id: u64,
    rng: Prng,
    last_event_flags: u16,
    feed: F,
    exec: E,
    strategy: S,
    trace: T,
    head_present: bool,
    head_header: EventHeader,
    head_payload: Vec<u8>,
    current_header: EventHeader,
    current_payload: Vec<u8>,
    result_header: EventHeader,
    result_payload: Vec<u8>,
    queue: ActionQueue,
    history: HistoryRing,
    arena: Vec<u8>,
    canonical_buf: Vec<u8>,
    action_sort_scratch: Vec<usize>,
    feed_token_scratch: Vec<u8>,
    exec_state_scratch: Vec<u8>,
}

/// ステップの中の失敗。`(終了コード, 詳細コード)`。
type Failure = (i32, i32);

fn port_failure(error: PortError) -> Failure {
    (ERR_PORT_FAILURE, error.0)
}

impl<F, E, S> Kernel<F, E, S, NoTraceSink>
where
    F: EventFeedPort,
    E: ExecutionPort,
    S: Strategy,
{
    /// トレース出力なしのカーネルを作成する（F-001）。
    /// `config.trace_enabled` が true の場合は `ERR_INVALID_ARGUMENT` を返す。
    pub fn create(config: KernelConfig, feed: F, exec: E, strategy: S) -> Result<Self, i32> {
        Kernel::create_with_trace(config, feed, exec, strategy, NoTraceSink)
    }
}

impl<F, E, S, T> Kernel<F, E, S, T>
where
    F: EventFeedPort,
    E: ExecutionPort,
    S: Strategy,
    T: TraceSink,
{
    /// `TraceSink` を伴うカーネルを作成する（F-001）。確保は、この関数に限る。失敗時はハンドルを作らない。
    pub fn create_with_trace(
        config: KernelConfig,
        mut feed: F,
        exec: E,
        strategy: S,
        mut trace: T,
    ) -> Result<Self, i32> {
        config.validate(1, trace.is_present())?;
        let payload_len =
            usize::try_from(config.payload_record_size).map_err(|_| ERR_CAPACITY_EXCEEDED)?;
        let queue_len =
            usize::try_from(config.action_queue_capacity).map_err(|_| ERR_CAPACITY_EXCEEDED)?;
        let history_len =
            usize::try_from(config.history_ring_len).map_err(|_| ERR_CAPACITY_EXCEEDED)?;
        let arena_len = usize::try_from(config.arena_size).map_err(|_| ERR_CAPACITY_EXCEEDED)?;
        let snapshot_len = config
            .max_snapshot_bytes(1)
            .and_then(|n| usize::try_from(n).ok())
            .ok_or(ERR_CAPACITY_EXCEEDED)?;
        let feed_token_len =
            usize::try_from(config.feed_token_max_bytes).map_err(|_| ERR_CAPACITY_EXCEEDED)?;
        let exec_state_len =
            usize::try_from(config.exec_state_max_bytes).map_err(|_| ERR_CAPACITY_EXCEEDED)?;

        let queue = ActionQueue::new(queue_len, payload_len).ok_or(ERR_CAPACITY_EXCEEDED)?;
        let history = HistoryRing::new(history_len, payload_len).ok_or(ERR_CAPACITY_EXCEEDED)?;

        let mut head_header = EventHeader::default();
        let mut head_payload = vec![0u8; payload_len];
        let head_present =
            intake(&mut feed, &mut head_header, &mut head_payload).map_err(|(code, _)| code)?;

        if config.trace_enabled {
            trace
                .write_header(&TraceHeader::new(config.hash_interval))
                .map_err(|_| ERR_PORT_FAILURE)?;
        }

        Ok(Self {
            inner: RefCell::new(Inner {
                config,
                state: HandleState::Ready,
                now_ns: CLOCK_UNSET_NS,
                processed_event_count: 0,
                next_action_sequence_id: 0,
                rng: Prng::from_seed(config.prng_seed),
                last_event_flags: 0,
                feed,
                exec,
                strategy,
                trace,
                head_present,
                head_header,
                head_payload,
                current_header: EventHeader::default(),
                current_payload: vec![0; payload_len],
                result_header: EventHeader::default(),
                result_payload: vec![0; payload_len],
                queue,
                history,
                arena: vec![0; arena_len],
                canonical_buf: vec![0; snapshot_len],
                action_sort_scratch: vec![0; queue_len],
                feed_token_scratch: vec![0; feed_token_len],
                exec_state_scratch: vec![0; exec_state_len],
            }),
        })
    }

    /// 次の1イベントを処理する（F-002）。
    pub fn step(&self) -> StepResult {
        match self.inner.try_borrow_mut() {
            Ok(mut inner) => inner.step(),
            Err(_) => reentrant(),
        }
    }

    /// 停止条件が成立するまで `step` を繰り返す（F-003）。
    pub fn run(&self, limit: RunLimit) -> StepResult {
        match self.inner.try_borrow_mut() {
            Ok(mut inner) => inner.run(limit),
            Err(_) => reentrant(),
        }
    }

    /// 戦略領域へ初期値を書き込む（F-011）。最初のステップより前に限る。
    ///
    /// `offset` は `StrategyArena` の先頭からの位置で、8の倍数。失敗しても状態を変更しない。
    pub fn init_strategy_arena(&self, offset: u64, bytes: &[u8]) -> i32 {
        match self.inner.try_borrow_mut() {
            Ok(mut inner) => inner.init_strategy_arena(offset, bytes),
            Err(_) => ERR_REENTRANT,
        }
    }

    /// 戦略領域の内容を、呼び出し側のバッファへコピーする（F-012）。`out.len()` バイトを読む。
    pub fn read_strategy_arena(&self, offset: u64, out: &mut [u8]) -> i32 {
        match self.inner.try_borrow() {
            Ok(inner) => inner.read_strategy_arena(offset, out),
            Err(_) => ERR_REENTRANT,
        }
    }

    /// 保存を伴わずに、現在の全セクション正準バイト列（EK-DS-001 2.5節）の SHA-256 を計算する。
    /// `seekable` でないフィードがあっても計算できる。
    pub fn state_digest(&self) -> Result<[u8; 32], i32> {
        let mut inner = self.inner.try_borrow_mut().map_err(|_| ERR_REENTRANT)?;
        if inner.state == HandleState::Poisoned {
            return Err(ERR_POISONED);
        }
        let len = inner.build_canonical(true).map_err(|(code, _)| code)?;
        let bytes = inner.canonical_buf.get(..len).ok_or(ERR_PANIC)?;
        Ok(sha256(bytes))
    }

    /// 現在の正準セクション列を `out` へコピーし、書き込んだバイト数を返す。
    /// `include_config_and_history` が false の場合、ステップハッシュ対象（セクション 2,3,4,5,6,8,9）を書く。
    pub fn write_canonical_state(
        &self,
        include_config_and_history: bool,
        out: &mut [u8],
    ) -> Result<usize, i32> {
        let mut inner = self.inner.try_borrow_mut().map_err(|_| ERR_REENTRANT)?;
        if inner.state == HandleState::Poisoned {
            return Err(ERR_POISONED);
        }
        let len = inner
            .build_canonical(include_config_and_history)
            .map_err(|(code, _)| code)?;
        let Some(src) = inner.canonical_buf.get(..len) else {
            return Err(ERR_PANIC);
        };
        let Some(dst) = out.get_mut(..len) else {
            return Err(ERR_INVALID_ARGUMENT);
        };
        dst.copy_from_slice(src);
        Ok(len)
    }

    /// ハンドルの状態。ステップの実行中に呼ばれた場合は `Running`。
    pub fn state(&self) -> HandleState {
        self.inner
            .try_borrow()
            .map_or(HandleState::Running, |inner| inner.state)
    }

    /// ポートと戦略を読み取る。ステップの実行中は `ERR_REENTRANT`。
    pub fn inspect<R>(&self, read: impl FnOnce(&F, &E, &S) -> R) -> Result<R, i32> {
        let inner = self.inner.try_borrow().map_err(|_| ERR_REENTRANT)?;
        Ok(read(&inner.feed, &inner.exec, &inner.strategy))
    }

    /// トレース出力先を読み取る。ステップの実行中は `ERR_REENTRANT`。
    pub fn inspect_trace<R>(&self, read: impl FnOnce(&T) -> R) -> Result<R, i32> {
        let inner = self.inner.try_borrow().map_err(|_| ERR_REENTRANT)?;
        Ok(read(&inner.trace))
    }

    /// 設定。ステップの実行中は `ERR_REENTRANT`。
    pub fn config(&self) -> Result<KernelConfig, i32> {
        let inner = self.inner.try_borrow().map_err(|_| ERR_REENTRANT)?;
        Ok(inner.config)
    }

    /// ハンドルを破棄し、ポートと戦略を返す（F-007）。
    pub fn destroy(self) -> (F, E, S) {
        let inner = self.inner.into_inner();
        (inner.feed, inner.exec, inner.strategy)
    }

    /// ハンドルを破棄し、トレース出力先を含むポートと戦略を返す（F-007）。
    pub fn destroy_with_trace(self) -> (F, E, S, T) {
        let inner = self.inner.into_inner();
        (inner.feed, inner.exec, inner.strategy, inner.trace)
    }
}

fn reentrant() -> StepResult {
    StepResult {
        status: ERR_REENTRANT,
        detail: 0,
        processed_event_count: 0,
        now_ns: 0,
    }
}

/// 外部フィードから次のイベントを取り込む。`Ok(false)` は終端。
fn intake<F: EventFeedPort>(
    feed: &mut F,
    header: &mut EventHeader,
    payload: &mut [u8],
) -> Result<bool, Failure> {
    let Some(timestamp_ns) = feed.peek_next_timestamp().map_err(port_failure)? else {
        return Ok(false);
    };
    payload.fill(0);
    feed.pop_next(header, payload).map_err(port_failure)?;
    // DD に定めのない検査（縦切り計画 H4 の記録を参照）。
    if header.timestamp_ns != timestamp_ns
        || header.stream_id != FEED_STREAM_ID
        || !event_type::is_market(header.event_type)
    {
        return Err((ERR_PORT_FAILURE, 0));
    }
    header.payload_idx = 0;
    Ok(true)
}

impl<F, E, S, T> Inner<F, E, S, T>
where
    F: EventFeedPort,
    E: ExecutionPort,
    S: Strategy,
    T: TraceSink,
{
    fn result(&self, status: i32, detail: i32) -> StepResult {
        StepResult {
            status,
            detail,
            processed_event_count: self.processed_event_count,
            now_ns: self.now_ns,
        }
    }

    fn build_canonical(&mut self, include_config_and_history: bool) -> Result<usize, Failure> {
        let Inner {
            config,
            now_ns,
            processed_event_count,
            next_action_sequence_id,
            rng,
            feed,
            exec,
            head_present,
            head_header,
            head_payload,
            queue,
            history,
            arena,
            canonical_buf,
            action_sort_scratch,
            feed_token_scratch,
            exec_state_scratch,
            ..
        } = self;
        let inputs = CanonicalInputs {
            config,
            now_ns: *now_ns,
            processed_event_count: *processed_event_count,
            next_action_sequence_id: *next_action_sequence_id,
            rng,
            arena,
            head_present: *head_present,
            head_header: *head_header,
            head_payload,
            feed_stream_id: FEED_STREAM_ID,
            queue,
            history,
            feed,
            exec,
        };
        let mut scratch = CanonicalScratch {
            buf: canonical_buf,
            action_sort: action_sort_scratch,
            feed_token: feed_token_scratch,
            exec_state: exec_state_scratch,
        };
        write_canonical_sections(&inputs, include_config_and_history, &mut scratch)
    }

    /// `StrategyArena` の内側の範囲を、`arena` 全体の添字の範囲にする。範囲外なら `None`。
    fn arena_range(&self, offset: u64, len: usize) -> Option<core::ops::Range<usize>> {
        if !offset.is_multiple_of(8) {
            return None;
        }
        let end = offset.checked_add(u64::try_from(len).ok()?)?;
        if end > self.config.strategy_arena_size {
            return None;
        }
        let base = self.config.strategy_arena_offset;
        let start = usize::try_from(base.checked_add(offset)?).ok()?;
        let end = usize::try_from(base.checked_add(end)?).ok()?;
        Some(start..end)
    }

    fn init_strategy_arena(&mut self, offset: u64, bytes: &[u8]) -> i32 {
        match self.state {
            HandleState::Poisoned => return ERR_POISONED,
            HandleState::Ready if self.processed_event_count == 0 => {}
            _ => return ERR_INVALID_ARGUMENT,
        }
        let Some(range) = self.arena_range(offset, bytes.len()) else {
            return ERR_INVALID_ARGUMENT;
        };
        match self.arena.get_mut(range) {
            Some(target) => {
                target.copy_from_slice(bytes);
                OK
            }
            None => ERR_INVALID_ARGUMENT,
        }
    }

    fn read_strategy_arena(&self, offset: u64, out: &mut [u8]) -> i32 {
        match self.state {
            HandleState::Poisoned => return ERR_POISONED,
            HandleState::Ready | HandleState::Finished => {}
            HandleState::Running => return ERR_REENTRANT,
        }
        let Some(range) = self.arena_range(offset, out.len()) else {
            return ERR_INVALID_ARGUMENT;
        };
        match self.arena.get(range) {
            Some(source) => {
                out.copy_from_slice(source);
                OK
            }
            None => ERR_INVALID_ARGUMENT,
        }
    }

    fn poison(&mut self, (status, detail): Failure) -> StepResult {
        self.state = HandleState::Poisoned;
        self.result(status, detail)
    }

    fn run(&mut self, limit: RunLimit) -> StepResult {
        let mut processed: u64 = 0;
        loop {
            let result = self.step();
            if result.status != OK {
                return result;
            }
            processed += 1;
            if self.last_event_flags & flags::SNAPSHOT_TRIGGER != 0 {
                return self.result(RUN_SNAPSHOT_REQUESTED, 0);
            }
            if limit.max_events != 0 && processed >= limit.max_events {
                return self.result(RUN_LIMIT_REACHED, 0);
            }
        }
    }

    /// 手順表（EK-DD-001 3.1節）の手順1〜10。
    fn step(&mut self) -> StepResult {
        // 手順1: 状態の検査。
        match self.state {
            HandleState::Poisoned => return self.result(ERR_POISONED, 0),
            HandleState::Finished => return self.result(STEP_FINISHED, 0),
            HandleState::Ready | HandleState::Running => {}
        }
        // 手順2: 最小のイベントを選ぶ。
        let from_feed = match (self.head_present, self.queue.peek_header()) {
            (false, None) => {
                self.state = HandleState::Finished;
                return self.result(STEP_FINISHED, 0);
            }
            (true, None) => true,
            (false, Some(_)) => false,
            (true, Some(queued)) => self.head_header.order_key() <= queued.order_key(),
        };
        let header = if from_feed {
            self.head_header
        } else {
            if !self
                .queue
                .pop_into(&mut self.current_header, &mut self.current_payload)
            {
                return self.poison((ERR_PANIC, 0));
            }
            self.current_header
        };
        // 手順3: 時刻の検証（防御的）。
        if header.timestamp_ns < self.now_ns {
            return self.poison((ERR_CAUSALITY_VIOLATION, 0));
        }
        // 手順4: 時刻を進める。
        self.now_ns = header.timestamp_ns;
        self.last_event_flags = header.flags;
        // 手順5〜7。
        if let Err(failure) = self.dispatch(from_feed, header) {
            return self.poison(failure);
        }
        self.processed_event_count += 1;
        // 手順8: 先頭の補充と検証。
        if from_feed && let Err(failure) = self.refill(header) {
            return self.poison(failure);
        }
        // 手順9: トレースと状態ハッシュ（EK-DD-001 3.1節）。
        if self.config.trace_enabled {
            let hash_present = self.config.hash_interval != 0
                && self
                    .processed_event_count
                    .is_multiple_of(self.config.hash_interval);
            let state_hash = if hash_present {
                let len = match self.build_canonical(false) {
                    Ok(len) => len,
                    Err(failure) => return self.poison(failure),
                };
                let Some(bytes) = self.canonical_buf.get(..len) else {
                    return self.poison((ERR_PANIC, 0));
                };
                xxh3_64(bytes)
            } else {
                0
            };
            let record = TraceRecord {
                timestamp_ns: header.timestamp_ns,
                sequence_id: header.sequence_id,
                stream_id: header.stream_id,
                event_type: header.event_type,
                event_flags: header.flags,
                state_hash,
                trace_flags: if hash_present {
                    TRACE_FLAG_HASH_PRESENT
                } else {
                    0
                },
                reserved: [0; 6],
            };
            if let Err(err) = self.trace.write(&record) {
                return self.poison(port_failure(err));
            }
        }
        // 手順10。
        self.result(OK, 0)
    }

    /// 手順5〜7: `ExecutionPort`、履歴、戦略への配送。
    fn dispatch(&mut self, from_feed: bool, header: EventHeader) -> Result<(), Failure> {
        let Inner {
            config,
            now_ns,
            next_action_sequence_id,
            rng,
            exec,
            strategy,
            head_payload,
            current_payload,
            result_header,
            result_payload,
            queue,
            history,
            arena,
            ..
        } = self;
        let now_ns = *now_ns;
        let payload: &[u8] = if from_feed {
            head_payload
        } else {
            current_payload
        };
        let event = EventRecord { header, payload };
        let kind = header.event_type;
        let market = event_type::is_market(kind);

        // 手順5: `ExecutionPort` の呼び出しと、結果の検査。
        if market || kind == event_type::ACTION_ARRIVAL {
            if market {
                exec.on_market_event(event, HistoryView::new(history), rng)
                    .map_err(port_failure)?;
            } else {
                exec.on_arrival(event, rng).map_err(port_failure)?;
            }
            loop {
                *result_header = EventHeader::default();
                result_payload.fill(0);
                if !exec
                    .poll_result(result_header, result_payload)
                    .map_err(port_failure)?
                {
                    break;
                }
                let result_kind = result_header.event_type;
                if result_kind != event_type::ORDER_ACK && result_kind != event_type::FILL {
                    return Err((ERR_PORT_FAILURE, 0));
                }
                if result_header.timestamp_ns < now_ns {
                    return Err((ERR_CAUSALITY_VIOLATION, 0));
                }
                result_header.stream_id = ACTION_STREAM_ID;
                result_header.sequence_id = *next_action_sequence_id;
                result_header.payload_idx = 0;
                let source: &[u8] = result_payload;
                if !queue.push_with(*result_header, |stored| copy_bytes(stored, source)) {
                    return Err((ERR_RESULT_OVERFLOW, 0));
                }
                *next_action_sequence_id += 1;
            }
        }

        // 手順6: 市場イベントだけを履歴へ追加する。
        if market {
            history.push(header, payload);
        }

        // 手順7: 戦略への通知。`ActionArrival` では呼ばない。
        if market || kind == event_type::ORDER_ACK || kind == event_type::FILL {
            let offset = usize::try_from(config.strategy_arena_offset).unwrap_or(usize::MAX);
            let size = usize::try_from(config.strategy_arena_size).unwrap_or(0);
            let strategy_arena = arena
                .get_mut(offset..offset.saturating_add(size))
                .unwrap_or_default();
            let mut context = Context {
                now_ns,
                history: HistoryView::new(history),
                strategy_arena,
                rng,
                queue,
                next_action_sequence_id,
            };
            let status = strategy.on_event(&mut context, event);
            if status != 0 {
                return Err((
                    ERR_STRATEGY_FAILURE,
                    i32::try_from(status).unwrap_or(i32::MAX),
                ));
            }
        }
        Ok(())
    }

    /// 手順8: 外部フィードの先頭を補充し、今のイベントと比べて検証する。
    fn refill(&mut self, current: EventHeader) -> Result<(), Failure> {
        self.head_present = intake(
            &mut self.feed,
            &mut self.head_header,
            &mut self.head_payload,
        )?;
        if !self.head_present {
            return Ok(());
        }
        let next = self.head_header;
        if next.timestamp_ns < current.timestamp_ns {
            return Err((ERR_CAUSALITY_VIOLATION, 0));
        }
        if next.timestamp_ns == current.timestamp_ns && next.sequence_id <= current.sequence_id {
            return Err((ERR_SEQUENCE_VIOLATION, 0));
        }
        Ok(())
    }
}
