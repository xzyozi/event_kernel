//! PRNG の配線（TR-017、EK-DD-001 2.6節）。戦略と `ExecutionPort` は、同じ `core` の PRNG を
//! 使う。呼び出し順序が決定論的なので、同じシードからは同じ乱数が同じ順に配られる。

mod common;

use common::*;
use ek_core::*;

/// 市場イベントごとに1つ引く `ExecutionPort`。
struct DrawingExec {
    draws: Vec<u64>,
}

impl ExecutionPort for DrawingExec {
    fn on_arrival(&mut self, _event: EventRecord<'_>, _rng: &mut Prng) -> Result<(), PortError> {
        Ok(())
    }

    fn on_market_event(
        &mut self,
        _event: EventRecord<'_>,
        _history: HistoryView<'_>,
        rng: &mut Prng,
    ) -> Result<(), PortError> {
        if self.draws.len() < self.draws.capacity() {
            self.draws.push(rng.next_u64());
        }
        Ok(())
    }

    fn poll_result(&mut self, _h: &mut EventHeader, _p: &mut [u8]) -> Result<bool, PortError> {
        Ok(false)
    }
}

/// 市場イベントごとに1つ引く戦略。
struct DrawingStrategy {
    draws: Vec<u64>,
}

impl Strategy for DrawingStrategy {
    fn on_event(&mut self, context: &mut Context<'_>, _event: EventRecord<'_>) -> StrategyStatus {
        if self.draws.len() < self.draws.capacity() {
            self.draws.push(context.rng().next_u64());
        }
        0
    }
}

fn run_with_seed(seed: u64) -> (Vec<u64>, Vec<u64>) {
    let mut cfg = config();
    cfg.prng_seed = seed;
    let kernel = Kernel::create(
        cfg,
        VecFeed::ticks(&[1, 2, 3, 4]),
        DrawingExec {
            draws: Vec::with_capacity(8),
        },
        DrawingStrategy {
            draws: Vec::with_capacity(8),
        },
    )
    .unwrap();
    assert_eq!(kernel.run(RunLimit::default()).status, STEP_FINISHED);
    let (_, exec, strategy) = kernel.destroy();
    (exec.draws, strategy.draws)
}

#[test]
fn exec_port_draws_before_strategy_from_one_shared_stream() {
    let (exec, strategy) = run_with_seed(99);
    let mut reference = Prng::from_seed(99);
    let expected: Vec<u64> = (0..8).map(|_| reference.next_u64()).collect();
    // 各ステップで、ExecutionPort が先に引き、次に戦略が引く。
    assert_eq!(
        exec,
        vec![expected[0], expected[2], expected[4], expected[6]]
    );
    assert_eq!(
        strategy,
        vec![expected[1], expected[3], expected[5], expected[7]]
    );
}

#[test]
fn same_seed_reproduces_and_different_seed_differs() {
    assert_eq!(run_with_seed(5), run_with_seed(5));
    assert_ne!(run_with_seed(5), run_with_seed(6));
}
