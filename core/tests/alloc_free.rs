//! AC-05（TR-002、QG-005）: `create` の完了後、`step` と `run` の間にヒープの確保も解放も起きない。

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use common::*;
use ek_core::*;

struct CountingAllocator;

thread_local! {
    // テストのスレッドごとに数える（並列実行される別テストと干渉しない）。
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static ALLOCS: Cell<u64> = const { Cell::new(0) };
    static DEALLOCS: Cell<u64> = const { Cell::new(0) };
}

fn counting() -> bool {
    COUNTING.try_with(Cell::get).unwrap_or(false)
}

fn add_alloc() {
    let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
}

fn add_dealloc() {
    let _ = DEALLOCS.try_with(|c| c.set(c.get() + 1));
}

fn reset_counts() {
    ALLOCS.with(|c| c.set(0));
    DEALLOCS.with(|c| c.set(0));
}

fn alloc_count() -> u64 {
    ALLOCS.with(Cell::get)
}

fn dealloc_count() -> u64 {
    DEALLOCS.with(Cell::get)
}

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if counting() {
            add_alloc();
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if counting() {
            add_dealloc();
        }
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if counting() {
            add_alloc();
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

/// 毎イベントでアクションを出し、`OrderAck` と `Fill` も受け取る戦略。確保しない。
struct Busy {
    events: u64,
}

impl Strategy for Busy {
    fn on_event(&mut self, context: &mut Context<'_>, event: EventRecord<'_>) -> StrategyStatus {
        self.events += 1;
        context.strategy_arena()[0] = context.strategy_arena()[0].wrapping_add(1);
        if event_type::is_market(event.header.event_type) {
            let action = Action {
                action_kind: 1,
                latency_ns: 3,
                payload: &[1, 2, 3],
            };
            let _ = context.submit_action(&action);
        }
        0
    }
}

#[test]
fn ac05_no_heap_allocation_after_create() {
    let timestamps: Vec<i64> = (0..2_000).map(|i| i * 2).collect();
    let mut feed = VecFeed::ticks(&timestamps);
    // 同一時刻のイベントも含める。
    feed.events.push(tick(4_000, 10_000));
    feed.events.push(tick(4_000, 10_001));
    let kernel = Kernel::create(config(), feed, FillNextExec::new(), Busy { events: 0 }).unwrap();

    reset_counts();
    COUNTING.with(|c| c.set(true));
    let first = kernel.step();
    let rest = kernel.run(RunLimit { max_events: 500 });
    let last = kernel.run(RunLimit::default());
    COUNTING.with(|c| c.set(false));

    assert_eq!(first.status, OK);
    assert_eq!(rest.status, RUN_LIMIT_REACHED);
    assert_eq!(last.status, STEP_FINISHED);
    // 市場2,002件、到達、OrderAck、Fill を処理している。
    assert!(
        last.processed_event_count > 6_000,
        "{}",
        last.processed_event_count
    );
    assert_eq!(alloc_count(), 0, "確保の回数");
    assert_eq!(dealloc_count(), 0, "解放の回数");

    // 自己検査: 数える区間での確保が、実際に数えられることを確かめる。
    COUNTING.with(|c| c.set(true));
    let probe = std::hint::black_box(vec![0u8; 16]);
    drop(probe);
    COUNTING.with(|c| c.set(false));
    assert_eq!(alloc_count(), 1);
    assert_eq!(dealloc_count(), 1);
}

struct FixedTraceSink {
    records: Vec<TraceRecord>,
}

impl FixedTraceSink {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            records: Vec::with_capacity(capacity),
        }
    }
}

impl TraceSink for FixedTraceSink {
    fn write_header(&mut self, _header: &TraceHeader) -> Result<(), PortError> {
        Ok(())
    }

    fn write(&mut self, record: &TraceRecord) -> Result<(), PortError> {
        if self.records.len() < self.records.capacity() {
            self.records.push(*record);
        }
        Ok(())
    }
}

#[test]
fn ac05_no_heap_allocation_with_trace_and_state_hash() {
    let timestamps: Vec<i64> = (0..1_000).map(|i| i * 2).collect();
    let feed = VecFeed::ticks(&timestamps);
    let mut cfg = config();
    cfg.trace_enabled = true;
    cfg.hash_interval = 10;

    let kernel = Kernel::create_with_trace(
        cfg,
        feed,
        FillNextExec::new(),
        Busy { events: 0 },
        FixedTraceSink::with_capacity(4_096),
    )
    .unwrap();

    reset_counts();
    COUNTING.with(|c| c.set(true));
    let first = kernel.step();
    let rest = kernel.run(RunLimit { max_events: 500 });
    let last = kernel.run(RunLimit::default());
    let digest = kernel.state_digest().unwrap();
    COUNTING.with(|c| c.set(false));

    assert_eq!(first.status, OK);
    assert_eq!(rest.status, RUN_LIMIT_REACHED);
    assert_eq!(last.status, STEP_FINISHED);
    assert_ne!(digest, [0u8; 32]);
    assert_eq!(alloc_count(), 0, "トレース・ハッシュ時の確保回数");
    assert_eq!(dealloc_count(), 0, "トレース・ハッシュ時の解放回数");
}

#[test]
fn ac05_no_heap_allocation_with_multi_feed_and_trace() {
    let f0 = VecFeed::new((0..500).map(|i| tick(i * 2, i as u64)).collect());
    let f1 = VecFeed::new(
        (0..500)
            .map(|i| EventHeader {
                timestamp_ns: i * 2,
                sequence_id: i as u64,
                stream_id: 1,
                event_type: event_type::BAR,
                flags: 0,
                payload_idx: 0,
            })
            .collect(),
    );
    let f2 = VecFeed::new(
        (0..250)
            .map(|i| EventHeader {
                timestamp_ns: i * 4 + 1,
                sequence_id: i as u64,
                stream_id: 2,
                event_type: event_type::INDICATOR,
                flags: 0,
                payload_idx: 0,
            })
            .collect(),
    );
    let mut cfg = config();
    cfg.trace_enabled = true;
    cfg.hash_interval = 10;

    let kernel = Kernel::create_multi_with_trace(
        cfg,
        vec![f0, f1, f2],
        FillNextExec::new(),
        Busy { events: 0 },
        FixedTraceSink::with_capacity(4_096),
    )
    .unwrap();

    reset_counts();
    COUNTING.with(|c| c.set(true));
    let res = kernel.run(RunLimit::default());
    let digest = kernel.state_digest().unwrap();
    COUNTING.with(|c| c.set(false));

    assert_eq!(res.status, STEP_FINISHED);
    assert_ne!(digest, [0u8; 32]);
    assert_eq!(alloc_count(), 0, "複数フィード時の確保回数");
    assert_eq!(dealloc_count(), 0, "複数フィード時の解放回数");
}
