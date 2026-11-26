//! AC-01（TR-004）と、連番の検証（TR-005）。

mod common;

use common::*;
use ek_core::*;

#[test]
fn ac01_backward_timestamp_is_not_processed_and_poisons() {
    let kernel = kernel(VecFeed::ticks(&[10, 20, 15, 30]), RecordingStrategy::new());

    assert_eq!(kernel.step().status, OK);
    // 20 を処理したステップの手順8で、次の 15 の逆行を検出する。
    let result = kernel.step();
    assert_eq!(result.status, ERR_CAUSALITY_VIOLATION);
    assert_eq!(result.processed_event_count, 2);
    assert_eq!(result.now_ns, 20);
    assert_eq!(kernel.state(), HandleState::Poisoned);

    // 逆行したイベント 15 は、戦略に届いていない。
    let times: Vec<i64> = seen(&kernel).iter().map(|s| s.1).collect();
    assert_eq!(times, vec![10, 20]);

    // 以後の操作は拒否される。
    let result = kernel.step();
    assert_eq!(result.status, ERR_POISONED);
    assert_eq!(kernel.run(RunLimit::default()).status, ERR_POISONED);
}

#[test]
fn same_timestamp_without_increasing_sequence_is_rejected() {
    let feed = VecFeed::new(vec![tick(10, 0), tick(10, 0), tick(20, 1)]);
    let kernel = kernel(feed, RecordingStrategy::new());
    let result = kernel.step();
    assert_eq!(result.status, ERR_SEQUENCE_VIOLATION);
    assert_eq!(result.processed_event_count, 1);
    assert_eq!(kernel.state(), HandleState::Poisoned);
}

#[test]
fn same_timestamp_with_increasing_sequence_is_accepted() {
    let feed = VecFeed::new(vec![tick(10, 0), tick(10, 1), tick(10, 5)]);
    let kernel = kernel(feed, RecordingStrategy::new());
    let result = kernel.run(RunLimit::default());
    assert_eq!(result.status, STEP_FINISHED);
    assert_eq!(result.processed_event_count, 3);
}

#[test]
fn feed_failure_during_refill_poisons() {
    let mut feed = VecFeed::ticks(&[10, 20]);
    feed.fail_at = Some(1);
    let kernel = kernel(feed, RecordingStrategy::new());
    let result = kernel.step();
    assert_eq!(result.status, ERR_PORT_FAILURE);
    assert_eq!(result.detail, 7);
    assert_eq!(kernel.state(), HandleState::Poisoned);
}
