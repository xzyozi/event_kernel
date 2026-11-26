//! AC-05（TR-002、QG-005）: `create` の完了後、`step` と `run` の間にヒープの確保も解放も起きない。

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};

use common::*;
use ek_core::*;

struct CountingAllocator;

static ALLOCS: AtomicU64 = AtomicU64::new(0);
static DEALLOCS: AtomicU64 = AtomicU64::new(0);

thread_local! {
    // テストのスレッドだけを数える（テストハーネスの別スレッドの確保を除く）。
    static COUNTING: Cell<bool> = const { Cell::new(false) };
}

fn counting() -> bool {
    COUNTING.try_with(Cell::get).unwrap_or(false)
}

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if counting() {
            ALLOCS.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if counting() {
            DEALLOCS.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if counting() {
            ALLOCS.fetch_add(1, Ordering::SeqCst);
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
            let action = Action { action_kind: 1, latency_ns: 3, payload: &[1, 2, 3] };
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

    COUNTING.with(|c| c.set(true));
    let first = kernel.step();
    let rest = kernel.run(RunLimit { max_events: 500 });
    let last = kernel.run(RunLimit::default());
    COUNTING.with(|c| c.set(false));

    assert_eq!(first.status, OK);
    assert_eq!(rest.status, RUN_LIMIT_REACHED);
    assert_eq!(last.status, STEP_FINISHED);
    // 市場2,002件、到達、OrderAck、Fill を処理している。
    assert!(last.processed_event_count > 6_000, "{}", last.processed_event_count);
    assert_eq!(ALLOCS.load(Ordering::SeqCst), 0, "確保の回数");
    assert_eq!(DEALLOCS.load(Ordering::SeqCst), 0, "解放の回数");

    // 自己検査: 数える区間での確保が、実際に数えられることを確かめる。
    COUNTING.with(|c| c.set(true));
    let probe = std::hint::black_box(vec![0u8; 16]);
    drop(probe);
    COUNTING.with(|c| c.set(false));
    assert_eq!(ALLOCS.load(Ordering::SeqCst), 1);
    assert_eq!(DEALLOCS.load(Ordering::SeqCst), 1);
}