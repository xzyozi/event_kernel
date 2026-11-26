//! カーネルのハンドルと、ステップの手順（EK-DD-001 2.2節、3.1節、3.2節）。

use alloc::vec;
use alloc::vec::Vec;
use core::cell::RefCell;

use crate::config::KernelConfig;
use crate::event::{
    ACTION_PAYLOAD_OFFSET, ACTION_STREAM_ID, EventHeader, EventRecord, event_type, flags,
};
use crate::history::{HistoryRing, HistoryView};
use crate::ports::{EventFeedPort, ExecutionPort, PortError, Strategy};
use crate::queue::ActionQueue;
use crate::status::*;
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
pub struct Kernel<F, E, S> {
    inner: RefCell<Inner<F, E, S>>,
}

struct Inner<F, E, S> {
    config: KernelConfig,
    state: HandleState,
    now_ns: i64,
    processed_event_count: u64,
    next_action_sequence_id: u64,
    last_event_flags: u16,
    feed: F,
    exec: E,
    strategy: S,
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
}

/// ステップの中の失敗。`(終了コード, 詳細コード)`。
type Failure = (i32, i32);

fn port_failure(error: PortError) -> Failure {
    (ERR_PORT_FAILURE, error.0)
}

impl<F, E, S> Kernel<F, E, S>
where
    F: EventFeedPort,
    E: ExecutionPort,
    S: Strategy,
{
    /// カーネルを作成する（F-001）。確保は、この関数に限る。失敗時はハンドルを作らない。
    pub fn create(config: KernelConfig, mut feed: F, exec: E, strategy: S) -> Result<Self, i32> {
        config.validate(1)?;
        let payload_len =
            usize::try_from(config.payload_record_size).map_err(|_| ERR_CAPACITY_EXCEEDED)?;
        let queue_len =
            usize::try_from(config.action_queue_capacity).map_err(|_| ERR_CAPACITY_EXCEEDED)?;
        let history_len =
            usize::try_from(config.history_ring_len).map_err(|_| ERR_CAPACITY_EXCEEDED)?;
        let arena_len = usize::try_from(config.arena_size).map_err(|_| ERR_CAPACITY_EXCEEDED)?;
        let queue = ActionQueue::new(queue_len, payload_len).ok_or(ERR_CAPACITY_EXCEEDED)?;
        let history = HistoryRing::new(history_len, payload_len).ok_or(ERR_CAPACITY_EXCEEDED)?;

        let mut head_header = EventHeader::default();
        let mut head_payload = vec![0u8; payload_len];
        let head_present =
            intake(&mut feed, &mut head_header, &mut head_payload).map_err(|(code, _)| code)?;

        Ok(Self {
            inner: RefCell::new(Inner {
                config,
                state: HandleState::Ready,
                now_ns: CLOCK_UNSET_NS,
                processed_event_count: 0,
                next_action_sequence_id: 0,
                last_event_flags: 0,
                feed,
                exec,
                strategy,
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

impl<F, E, S> Inner<F, E, S>
where
    F: EventFeedPort,
    E: ExecutionPort,
    S: Strategy,
{
    fn result(&self, status: i32, detail: i32) -> StepResult {
        StepResult {
            status,
            detail,
            processed_event_count: self.processed_event_count,
            now_ns: self.now_ns,
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
        // 手順9（トレースとハッシュ）は、縦切りの範囲外。
        // 手順10。
        self.result(OK, 0)
    }

    /// 手順5〜7: `ExecutionPort`、履歴、戦略への配送。
    fn dispatch(&mut self, from_feed: bool, header: EventHeader) -> Result<(), Failure> {
        let Inner {
            config,
            now_ns,
            next_action_sequence_id,
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
                exec.on_market_event(event, HistoryView::new(history))
                    .map_err(port_failure)?;
            } else {
                exec.on_arrival(event).map_err(port_failure)?;
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
