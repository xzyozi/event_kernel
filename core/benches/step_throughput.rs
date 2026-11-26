//! `step` のスループットのベンチマーク雛形（縦切り計画 H1、QG-008）。
//!
//! 要件定義書 8章に近い入力: 1イベント = `EventHeader`（32バイト）+ ペイロード64バイトの SoA を、
//! あらかじめメモリ上に展開する。戦略は移動平均、閾値判定、100イベントに1回のアクション発行。
//! 縦切りの範囲外のため、10,000イベントに1回の状態ハッシュは含まない。外部フィードは1本で、
//! 2ストリームを統合した列として扱う。
//!
//! 実行例（vCPU 1 に固定）: `taskset -c 1 cargo bench -p event-kernel-core --bench step_throughput`
//! 環境変数: `EK_BENCH_EVENTS`（既定 10,000,000）、`EK_BENCH_RUNS`（既定 5、3以上）。

// 計測のための壁時計。カーネルの実装（src/）には使わない（要件定義書 6章1項は core の実装が対象）。
#![allow(clippy::disallowed_types)]

use std::hint::black_box;
use std::time::Instant;

use ek_core::*;

const PAYLOAD: usize = 64;
const MA_LEN: usize = 32;

struct SliceFeed<'a> {
    headers: &'a [EventHeader],
    payloads: &'a [u8],
    next: usize,
}

impl EventFeedPort for SliceFeed<'_> {
    #[inline]
    fn peek_next_timestamp(&mut self) -> Result<Option<i64>, PortError> {
        Ok(self.headers.get(self.next).map(|h| h.timestamp_ns))
    }

    #[inline]
    fn pop_next(&mut self, header: &mut EventHeader, payload: &mut [u8]) -> Result<(), PortError> {
        let h = self.headers.get(self.next).ok_or(PortError(1))?;
        let offset = self.next * PAYLOAD;
        let src = self.payloads.get(offset..offset + PAYLOAD).ok_or(PortError(2))?;
        *header = *h;
        payload.copy_from_slice(src);
        self.next += 1;
        Ok(())
    }
}

/// 到達したアクションを、次の市場イベントの価格で約定させる。
#[derive(Default)]
struct NextEventFill {
    pending: u32,
    fills_to_emit: u32,
    fill_time: i64,
    fills: u64,
}

impl ExecutionPort for NextEventFill {
    #[inline]
    fn on_arrival(&mut self, _event: EventRecord<'_>) -> Result<(), PortError> {
        self.pending += 1;
        Ok(())
    }

    #[inline]
    fn on_market_event(&mut self, event: EventRecord<'_>, _h: HistoryView<'_>) -> Result<(), PortError> {
        if self.pending > 0 {
            self.fills_to_emit = self.pending;
            self.pending = 0;
            self.fill_time = event.header.timestamp_ns;
        }
        Ok(())
    }

    #[inline]
    fn poll_result(&mut self, header: &mut EventHeader, _payload: &mut [u8]) -> Result<bool, PortError> {
        if self.fills_to_emit == 0 {
            return Ok(false);
        }
        self.fills_to_emit -= 1;
        self.fills += 1;
        header.timestamp_ns = self.fill_time;
        header.event_type = event_type::FILL;
        Ok(true)
    }
}

/// 移動平均、閾値判定、100イベントに1回のアクション発行。
struct BenchStrategy {
    window: [i64; MA_LEN],
    pos: usize,
    sum: i64,
    market_events: u64,
    signals: u64,
    fills: u64,
}

impl Strategy for BenchStrategy {
    #[inline]
    fn on_event(&mut self, context: &mut Context<'_>, event: EventRecord<'_>) -> StrategyStatus {
        if event.header.event_type == event_type::FILL {
            self.fills += 1;
            return 0;
        }
        let mut price = [0u8; 8];
        if let Some(bytes) = event.payload.get(..8) {
            price.copy_from_slice(bytes);
        }
        let price = i64::from_le_bytes(price);
        if let Some(slot) = self.window.get_mut(self.pos) {
            self.sum += price - *slot;
            *slot = price;
        }
        self.pos = (self.pos + 1) % MA_LEN;
        if price * (MA_LEN as i64) > self.sum {
            self.signals += 1;
        }
        self.market_events += 1;
        if self.market_events % 100 == 0 {
            let action = Action { action_kind: 1, latency_ns: 1, payload: &[] };
            let _ = context.submit_action(&action);
        }
        0
    }
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn main() {
    let events = env_usize("EK_BENCH_EVENTS", 10_000_000);
    let runs = env_usize("EK_BENCH_RUNS", 5).max(3);

    // 入力の展開（計測に含めない）。時刻は2ナノ秒刻みで、価格は決定的な疑似乱数。
    let mut headers = Vec::with_capacity(events);
    let mut payloads = vec![0u8; events * PAYLOAD];
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    for i in 0..events {
        headers.push(EventHeader {
            timestamp_ns: (i as i64) * 2,
            sequence_id: i as u64,
            stream_id: 0,
            event_type: event_type::TICK,
            flags: 0,
            payload_idx: i as u64,
        });
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let price = 10_000 + (x % 200) as i64;
        payloads[i * PAYLOAD..i * PAYLOAD + 8].copy_from_slice(&price.to_le_bytes());
    }

    let config = KernelConfig {
        history_ring_len: 64,
        arena_size: 4096,
        strategy_arena_offset: 0,
        strategy_arena_size: 4096,
        action_queue_capacity: 1024,
        payload_record_size: PAYLOAD as u32,
        price_scale_exp: -2,
        qty_scale_exp: 0,
        prng_seed: 1,
        exec_state_max_bytes: 64,
        feed_token_max_bytes: 16,
        hash_interval: 0,
        trace_enabled: false,
        max_total_bytes: 1 << 24,
    };

    let mut rates = Vec::with_capacity(runs);
    for run in 0..runs {
        let feed = SliceFeed { headers: &headers, payloads: &payloads, next: 0 };
        let strategy = BenchStrategy {
            window: [0; MA_LEN],
            pos: 0,
            sum: 0,
            market_events: 0,
            signals: 0,
            fills: 0,
        };
        let kernel = Kernel::create(config, feed, NextEventFill::default(), strategy)
            .unwrap_or_else(|code| panic!("create failed: {code}"));
        let start = Instant::now();
        let result = kernel.run(RunLimit::default());
        let elapsed = start.elapsed();
        assert_eq!(result.status, STEP_FINISHED, "run failed: {result:?}");
        let (_, exec, strategy) = kernel.destroy();
        let rate = result.processed_event_count as f64 / elapsed.as_secs_f64();
        println!(
            "run {run}: processed={} market={} fills={} signals={} elapsed={:.3}s rate={:.0} events/s",
            result.processed_event_count,
            strategy.market_events,
            exec.fills,
            black_box(strategy.signals),
            elapsed.as_secs_f64(),
            rate
        );
        rates.push(rate);
    }
    rates.sort_by(f64::total_cmp);
    let median = rates[rates.len() / 2];
    println!("median: {median:.0} events/s (runs={runs}, input={events} market events)");
    println!("NFR-03 (>= 10,000,000 events/s): {}", if median >= 1e7 { "met" } else { "not met" });
}