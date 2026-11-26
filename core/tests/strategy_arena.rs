//! F-011 `init_strategy_arena` と F-012 `read_strategy_arena`（TR-022、EK-DD-001 2.2節）。

mod common;

use std::cell::RefCell;
use std::rc::Rc;

use common::*;
use ek_core::*;

/// `arena_size` 64 のうち、先頭 8 バイトを State Arena の別用途に空け、
/// 残り 48 バイトを `StrategyArena` にする。
fn arena_config() -> KernelConfig {
    let mut c = config();
    c.strategy_arena_offset = 8;
    c.strategy_arena_size = 48;
    c
}

/// 各イベントで、`StrategyArena` の先頭を読んで 1 を足す戦略。
struct Counter {
    first_seen: Vec<u8>,
}

impl Strategy for Counter {
    fn on_event(&mut self, context: &mut Context<'_>, _event: EventRecord<'_>) -> StrategyStatus {
        let arena = context.strategy_arena();
        if self.first_seen.len() < self.first_seen.capacity() {
            self.first_seen.push(arena[0]);
        }
        arena[0] = arena[0].wrapping_add(1);
        0
    }
}

type CounterKernel = Kernel<VecFeed, FillNextExec, Counter>;

fn counter_kernel_with(feed: VecFeed) -> CounterKernel {
    Kernel::create(
        arena_config(),
        feed,
        FillNextExec::new(),
        Counter {
            first_seen: Vec::with_capacity(8),
        },
    )
    .unwrap()
}

fn counter_kernel(timestamps: &[i64]) -> CounterKernel {
    counter_kernel_with(VecFeed::ticks(timestamps))
}

fn read(kernel: &CounterKernel, offset: u64, len: usize) -> (i32, Vec<u8>) {
    let mut out = vec![0xEE; len];
    (kernel.read_strategy_arena(offset, &mut out), out)
}

#[test]
fn init_before_first_step_is_visible_to_strategy_and_read_back() {
    let kernel = counter_kernel(&[10, 20]);
    assert_eq!(kernel.init_strategy_arena(0, &[7, 0, 0, 0, 0, 0, 0, 0]), OK);
    assert_eq!(read(&kernel, 0, 8), (OK, vec![7, 0, 0, 0, 0, 0, 0, 0]));

    kernel.run(RunLimit::default());
    // 戦略は初期値 7 を読み、各イベントで 1 を足した。
    assert_eq!(
        kernel.inspect(|_, _, s| s.first_seen.clone()).unwrap(),
        vec![7, 8]
    );
    assert_eq!(read(&kernel, 0, 8).1[0], 9);
}

#[test]
fn offset_is_relative_to_strategy_arena_not_to_state_arena() {
    let kernel = counter_kernel(&[10]);
    assert_eq!(kernel.init_strategy_arena(8, &[1, 2, 3, 4, 5, 6, 7, 8]), OK);
    assert_eq!(read(&kernel, 0, 16).1[..8], [0; 8]);
    assert_eq!(read(&kernel, 8, 8).1, vec![1, 2, 3, 4, 5, 6, 7, 8]);
}

#[test]
fn invalid_arguments_are_rejected_without_changing_state() {
    let kernel = counter_kernel(&[10]);
    assert_eq!(kernel.init_strategy_arena(0, &[5; 8]), OK);
    // 8 の倍数でないオフセット、範囲外（48 バイトを超える）、オフセットの桁あふれ。
    for (offset, len) in [(4u64, 8usize), (40, 16), (48, 8), (u64::MAX - 7, 16)] {
        assert_eq!(
            kernel.init_strategy_arena(offset, &vec![9; len]),
            ERR_INVALID_ARGUMENT
        );
        assert_eq!(read(&kernel, offset, len).0, ERR_INVALID_ARGUMENT);
    }
    assert_eq!(read(&kernel, 0, 8), (OK, vec![5; 8]));
    // 領域の末尾ちょうどまでは書ける。
    assert_eq!(kernel.init_strategy_arena(40, &[1; 8]), OK);
}
#[test]
fn init_is_rejected_after_the_first_step_but_read_still_works() {
    let kernel = counter_kernel(&[10, 20]);
    assert_eq!(kernel.step().status, OK);
    assert_eq!(kernel.init_strategy_arena(0, &[1; 8]), ERR_INVALID_ARGUMENT);
    assert_eq!(read(&kernel, 0, 8).1[0], 1);
    // Finished でも、読み出しはできて、書き込みは拒否される。
    assert_eq!(kernel.run(RunLimit::default()).status, STEP_FINISHED);
    assert_eq!(kernel.state(), HandleState::Finished);
    assert_eq!(read(&kernel, 0, 8).1[0], 2);
    assert_eq!(kernel.init_strategy_arena(0, &[1; 8]), ERR_INVALID_ARGUMENT);
}

#[test]
fn init_on_a_finished_empty_feed_is_rejected() {
    let kernel = counter_kernel(&[]);
    assert_eq!(kernel.step().status, STEP_FINISHED);
    assert_eq!(kernel.init_strategy_arena(0, &[1; 8]), ERR_INVALID_ARGUMENT);
}

#[test]
fn poisoned_handle_rejects_both() {
    let mut feed = VecFeed::ticks(&[10, 20]);
    feed.fail_at = Some(1);
    let kernel = counter_kernel_with(feed);
    assert_eq!(kernel.step().status, ERR_PORT_FAILURE);
    assert_eq!(kernel.state(), HandleState::Poisoned);
    assert_eq!(kernel.init_strategy_arena(0, &[1; 8]), ERR_POISONED);
    assert_eq!(read(&kernel, 0, 8).0, ERR_POISONED);
}

type ReenterCall = Rc<RefCell<Option<Box<dyn Fn() -> (i32, i32)>>>>;

struct Reenter {
    call: ReenterCall,
    observed: Vec<(i32, i32)>,
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
fn calls_from_a_callback_are_rejected_as_reentrant() {
    let slot: ReenterCall = Rc::default();
    let strategy = Reenter {
        call: slot.clone(),
        observed: Vec::new(),
    };
    let kernel = Rc::new(
        Kernel::create(
            arena_config(),
            VecFeed::ticks(&[10]),
            FillNextExec::new(),
            strategy,
        )
        .unwrap(),
    );
    let weak = Rc::downgrade(&kernel);
    *slot.borrow_mut() = Some(Box::new(move || {
        let kernel = weak.upgrade().unwrap();
        let mut out = [0u8; 8];
        (
            kernel.init_strategy_arena(0, &[1; 8]),
            kernel.read_strategy_arena(0, &mut out),
        )
    }));
    assert_eq!(kernel.step().status, OK);
    let observed = kernel.inspect(|_, _, s| s.observed.clone()).unwrap();
    assert_eq!(observed, vec![(ERR_REENTRANT, ERR_REENTRANT)]);
    *slot.borrow_mut() = None;
}
