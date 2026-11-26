//! F-001 `create`（TR-001、TR-018）。

mod common;

use common::*;
use ek_core::*;

fn create_with(config: KernelConfig, feed: VecFeed) -> Result<TestKernel, i32> {
    Kernel::create(config, feed, FillNextExec::new(), RecordingStrategy::new())
}

#[test]
fn create_succeeds_in_ready_state() {
    let kernel = create_with(config(), VecFeed::ticks(&[10, 20])).unwrap();
    assert_eq!(kernel.state(), HandleState::Ready);
    // 先頭の1件だけを取り込んでいる（TR-018）。
    assert_eq!(kernel.inspect(|f, _, _| f.next).unwrap(), 1);
    let result = kernel.step();
    assert_eq!(result.status, OK);
    assert_eq!(result.processed_event_count, 1);
}

#[test]
fn clock_is_unset_until_first_event() {
    let kernel = create_with(config(), VecFeed::ticks(&[])).unwrap();
    let result = kernel.step();
    assert_eq!(result.status, STEP_FINISHED);
    assert_eq!(result.now_ns, CLOCK_UNSET_NS);
    assert_eq!(result.processed_event_count, 0);
}

#[test]
fn invalid_fields_are_rejected() {
    let cases: [fn(&mut KernelConfig); 11] = [
        |c| c.history_ring_len = 0,
        |c| c.arena_size = 63,
        |c| c.strategy_arena_offset = 4,
        |c| c.strategy_arena_size = 12,
        |c| c.action_queue_capacity = 0,
        |c| c.payload_record_size = 8,
        |c| c.payload_record_size = 20,
        |c| c.price_scale_exp = 1,
        |c| c.qty_scale_exp = 1,
        |c| c.feed_token_max_bytes = 0,
        |c| c.trace_enabled = true,
    ];
    for (i, mutate) in cases.iter().enumerate() {
        let mut c = config();
        mutate(&mut c);
        let result = create_with(c, VecFeed::ticks(&[10]));
        assert_eq!(result.err(), Some(ERR_INVALID_ARGUMENT), "case {i}");
    }
}

#[test]
fn strategy_arena_outside_arena_is_rejected() {
    let mut c = config();
    c.strategy_arena_offset = 8;
    c.strategy_arena_size = 64;
    assert_eq!(create_with(c, VecFeed::ticks(&[10])).err(), Some(ERR_INVALID_ARGUMENT));
}

#[test]
fn capacity_over_limit_is_rejected() {
    let mut c = config();
    c.max_total_bytes = 1024;
    assert_eq!(create_with(c, VecFeed::ticks(&[10])).err(), Some(ERR_CAPACITY_EXCEEDED));
}

#[test]
fn capacity_overflow_is_rejected() {
    let mut c = config();
    c.arena_size = u64::MAX - 7;
    c.strategy_arena_size = 0;
    c.max_total_bytes = u64::MAX;
    assert_eq!(create_with(c, VecFeed::ticks(&[10])).err(), Some(ERR_CAPACITY_EXCEEDED));
}

#[test]
fn feed_failure_during_intake_is_port_failure() {
    let mut feed = VecFeed::ticks(&[10]);
    feed.fail_at = Some(0);
    assert_eq!(create_with(config(), feed).err(), Some(ERR_PORT_FAILURE));
}