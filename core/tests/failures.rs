//! 致命的エラー（TR-009、TR-019）、`run` の停止条件（TR-015）、再入（TR-014）。

mod common;

use std::cell::RefCell;
use std::rc::Rc;

use common::*;
use ek_core::*;

#[test]
fn strategy_failure_poisons_with_detail() {
    let mut strategy = RecordingStrategy::new();
    strategy.fail_on = Some((20, 42));
    let kernel = kernel(VecFeed::ticks(&[10, 20, 30]), strategy);
    let result = kernel.run(RunLimit::default());
    assert_eq!(result.status, ERR_STRATEGY_FAILURE);
    assert_eq!(result.detail, 42);
    assert_eq!(kernel.state(), HandleState::Poisoned);
}

fn run_with_exec(exec: FillNextExec) -> (StepResult, HandleState) {
    let mut strategy = RecordingStrategy::new();
    strategy.submit_on = Some((10, 5));
    let kernel = Kernel::create(config(), VecFeed::ticks(&[10, 20]), exec, strategy).unwrap();
    let result = kernel.run(RunLimit::default());
    (result, kernel.state())
}

#[test]
fn result_with_invalid_type_is_port_failure() {
    let mut exec = FillNextExec::new();
    exec.ack_type = event_type::TICK;
    assert_eq!(
        run_with_exec(exec),
        run_with_exec_expected(ERR_PORT_FAILURE)
    );
}

#[test]
fn result_in_the_past_is_causality_violation() {
    let mut exec = FillNextExec::new();
    exec.ack_time_offset = -1;
    assert_eq!(
        run_with_exec(exec),
        run_with_exec_expected(ERR_CAUSALITY_VIOLATION)
    );
}

fn run_with_exec_expected(status: i32) -> (StepResult, HandleState) {
    // 到達は時刻15。10 と 15 の間に外部イベントはないので、2件目の処理で失敗する。
    (
        StepResult {
            status,
            detail: 0,
            processed_event_count: 1,
            now_ns: 15,
        },
        HandleState::Poisoned,
    )
}

#[test]
fn run_stops_on_limit_finish_and_snapshot_trigger() {
    let kernel = kernel(VecFeed::ticks(&[1, 2, 3, 4, 5]), RecordingStrategy::new());
    let result = kernel.run(RunLimit { max_events: 2 });
    assert_eq!(
        (result.status, result.processed_event_count),
        (RUN_LIMIT_REACHED, 2)
    );
    let result = kernel.run(RunLimit { max_events: 0 });
    assert_eq!(
        (result.status, result.processed_event_count),
        (STEP_FINISHED, 5)
    );
    assert_eq!(kernel.state(), HandleState::Finished);
    // Finished での step は、状態を変えずに STEP_FINISHED を返す。
    assert_eq!(kernel.step().status, STEP_FINISHED);

    let mut events: Vec<EventHeader> = (1..=4).map(|t| tick(t, t as u64)).collect();
    events[1].flags = flags::SNAPSHOT_TRIGGER;
    let kernel = kernel_with(VecFeed::new(events));
    let result = kernel.run(RunLimit::default());
    assert_eq!(
        (result.status, result.processed_event_count),
        (RUN_SNAPSHOT_REQUESTED, 2)
    );
    assert_eq!(kernel.state(), HandleState::Ready);
}

fn kernel_with(feed: VecFeed) -> TestKernel {
    kernel(feed, RecordingStrategy::new())
}

/// コールバックから、ハンドルの操作を呼ぶ戦略。
struct Reenter {
    call: Rc<RefCell<Option<Box<dyn Fn() -> (i32, i32, HandleState)>>>>,
    observed: Vec<(i32, i32, HandleState)>,
}

impl Strategy for Reenter {
    fn on_event(&mut self, _context: &mut Context<'_>, _event: EventRecord<'_>) -> StrategyStatus {
        if let Some(call) = self.call.borrow().as_ref() {
            self.observed.push(call());
        }
        0
    }
}

#[test]
fn reentrant_calls_are_rejected() {
    let slot: Rc<RefCell<Option<Box<dyn Fn() -> (i32, i32, HandleState)>>>> = Rc::default();
    let strategy = Reenter {
        call: slot.clone(),
        observed: Vec::new(),
    };
    let kernel = Rc::new(
        Kernel::create(
            config(),
            VecFeed::ticks(&[10, 20]),
            FillNextExec::new(),
            strategy,
        )
        .unwrap(),
    );
    let weak = Rc::downgrade(&kernel);
    *slot.borrow_mut() = Some(Box::new(move || {
        let kernel = weak.upgrade().unwrap();
        (
            kernel.step().status,
            kernel.run(RunLimit::default()).status,
            kernel.state(),
        )
    }));

    assert_eq!(kernel.step().status, OK);
    let observed = kernel.inspect(|_, _, s| s.observed.clone()).unwrap();
    assert_eq!(
        observed,
        vec![(ERR_REENTRANT, ERR_REENTRANT, HandleState::Running)]
    );
    // 再入の拒否は、状態を変更しない。
    assert_eq!(kernel.state(), HandleState::Ready);
    *slot.borrow_mut() = None;
    assert_eq!(kernel.run(RunLimit::default()).status, STEP_FINISHED);
}
