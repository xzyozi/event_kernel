//! AC-02（TR-003、TR-007）と、配送の順序と履歴（TR-006、TR-020）。

mod common;

use common::*;
use ek_core::event_type::*;
use ek_core::*;

#[test]
fn ac02_same_time_external_before_system_and_no_early_fill() {
    let mut strategy = RecordingStrategy::new();
    strategy.submit_on = Some((10, 5));
    let kernel = kernel(VecFeed::ticks(&[10, 15, 20]), strategy);

    assert_eq!(kernel.run(RunLimit::default()).status, STEP_FINISHED);

    // 戦略に届いた順: 市場10、市場15（外部が先）、OrderAck15、市場20、Fill20。
    // ActionArrival は戦略に届かない（TR-020）。
    let seen = seen(&kernel);
    let order: Vec<(u16, i64, u32)> = seen.iter().map(|s| (s.0, s.1, s.3)).collect();
    assert_eq!(
        order,
        vec![
            (TICK, 10, 0),
            (TICK, 15, 0),
            (ORDER_ACK, 15, ACTION_STREAM_ID),
            (TICK, 20, 0),
            (FILL, 20, ACTION_STREAM_ID),
        ]
    );
    // システムストリームの連番: 到達0、OrderAck1、Fill2。
    let system: Vec<u64> = seen
        .iter()
        .filter(|s| s.3 == ACTION_STREAM_ID)
        .map(|s| s.2)
        .collect();
    assert_eq!(system, vec![1, 2]);
    assert_eq!(
        kernel.inspect(|_, _, s| s.submit_status.clone()).unwrap(),
        vec![OK]
    );

    // 提出時刻 10 + latency 5 より前に、結果は現れない。
    for s in seen.iter().filter(|s| s.0 == ORDER_ACK || s.0 == FILL) {
        assert!(s.1 >= 15);
    }
}

#[test]
fn execution_port_runs_before_strategy_without_current_event_in_history() {
    let mut strategy = RecordingStrategy::new();
    strategy.submit_on = Some((10, 5));
    let kernel = kernel(VecFeed::ticks(&[10, 15, 20]), strategy);
    kernel.run(RunLimit::default());

    let calls = kernel.inspect(|_, e, _| e.calls.clone()).unwrap();
    // ExecutionPort の記録: (種別, 時刻, 履歴の長さ, 履歴の最新)。履歴は今のイベントを含まない。
    assert_eq!(
        calls,
        vec![
            (TICK, 10, 0, None),
            (TICK, 15, 1, Some(10)),
            (ACTION_ARRIVAL, 15, 0, None),
            (TICK, 20, 2, Some(15)),
        ]
    );
    // 戦略から見た履歴は、市場イベントでは今のイベントを含み、結果のイベントでは含まない。
    let seen = seen(&kernel);
    let history: Vec<(u16, usize, Option<i64>)> = seen.iter().map(|s| (s.0, s.4, s.5)).collect();
    assert_eq!(
        history,
        vec![
            (TICK, 1, Some(10)),
            (TICK, 2, Some(15)),
            (ORDER_ACK, 2, Some(15)),
            (TICK, 3, Some(20)),
            (FILL, 3, Some(20)),
        ]
    );
}

#[test]
fn history_ring_overwrites_oldest() {
    let kernel = kernel(
        VecFeed::ticks(&[1, 2, 3, 4, 5, 6]),
        RecordingStrategy::new(),
    );
    kernel.run(RunLimit::default());
    let last = *seen(&kernel).last().unwrap();
    // history_ring_len は4。
    assert_eq!((last.4, last.5), (4, Some(6)));
}

#[test]
fn latency_must_be_positive_and_in_range() {
    for latency_ns in [0, -1, i64::MAX] {
        let mut strategy = RecordingStrategy::new();
        strategy.submit_on = Some((10, latency_ns));
        let kernel = kernel(VecFeed::ticks(&[10, 20]), strategy);
        assert_eq!(kernel.run(RunLimit::default()).status, STEP_FINISHED);
        let status = kernel.inspect(|_, _, s| s.submit_status.clone()).unwrap();
        assert_eq!(status, vec![ERR_INVALID_ARGUMENT], "latency {latency_ns}");
    }
}

#[test]
fn full_queue_returns_queue_full_without_poison() {
    struct Flood(Vec<i32>);
    impl Strategy for Flood {
        fn on_event(
            &mut self,
            context: &mut Context<'_>,
            _event: EventRecord<'_>,
        ) -> StrategyStatus {
            for _ in 0..17 {
                let action = Action {
                    action_kind: 0,
                    latency_ns: 1,
                    payload: &[],
                };
                self.0.push(context.submit_action(&action));
            }
            0
        }
    }
    let kernel = Kernel::create(
        config(),
        VecFeed::ticks(&[10]),
        FillNextExec::new(),
        Flood(Vec::new()),
    )
    .unwrap();
    // capacity 16 の17件目が ERR_QUEUE_FULL。
    let result = kernel.step();
    assert_eq!(result.status, OK);
    assert_eq!(kernel.state(), HandleState::Ready);
    let status = kernel.inspect(|_, _, s| s.0.clone()).unwrap();
    assert_eq!(status.iter().filter(|&&s| s == OK).count(), 16);
    assert_eq!(status.last(), Some(&ERR_QUEUE_FULL));
}
