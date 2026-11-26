//! `KernelConfig` と、その検証（EK-DD-001 2.3節）。

use crate::status::{ERR_CAPACITY_EXCEEDED, ERR_INVALID_ARGUMENT};

/// 登録できる外部フィードの数の上限（EK-DD-001 2.3節）。
pub const MAX_STREAMS: u32 = 4096;

/// カーネルの設定（EK-DD-001 2.3節 F-001）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KernelConfig {
    /// 履歴ウィンドウの要素数。1以上。
    pub history_ring_len: u32,
    /// State Arena 全体のバイト数。8の倍数。
    pub arena_size: u64,
    /// `StrategyArena` の先頭のオフセット。8の倍数。
    pub strategy_arena_offset: u64,
    /// `StrategyArena` のバイト数。8の倍数。
    pub strategy_arena_size: u64,
    /// アクションキューの容量。1以上。
    pub action_queue_capacity: u32,
    /// ペイロードのバイト数。16以上の8の倍数。
    pub payload_record_size: u32,
    /// 価格の小数点の位置。0以下。
    pub price_scale_exp: i32,
    /// 数量の小数点の位置。0以下。
    pub qty_scale_exp: i32,
    /// PRNG のシード（縦切りでは使わない）。
    pub prng_seed: u64,
    /// `EXEC_STATE` の最大バイト数。8の倍数。
    pub exec_state_max_bytes: u32,
    /// 位置トークンの最大バイト数。1以上。
    pub feed_token_max_bytes: u32,
    /// 状態ハッシュの間隔（縦切りでは使わない）。
    pub hash_interval: u64,
    /// トレースを出力するか。縦切りには `TraceSink` がないので、true は拒否する。
    pub trace_enabled: bool,
    /// `create` が確保する合計バイト数の上限。
    pub max_total_bytes: u64,
}

impl KernelConfig {
    /// 設定を検証し、導出される確保量を返す。
    pub(crate) fn validate(&self, stream_count: u32) -> Result<u64, i32> {
        let fields_ok = stream_count >= 1
            && self.history_ring_len >= 1
            && self.arena_size.is_multiple_of(8)
            && self.strategy_arena_offset.is_multiple_of(8)
            && self.strategy_arena_size.is_multiple_of(8)
            && self.action_queue_capacity >= 1
            && self.payload_record_size.is_multiple_of(8)
            && self.payload_record_size >= 16
            && self.price_scale_exp <= 0
            && self.qty_scale_exp <= 0
            && self.exec_state_max_bytes.is_multiple_of(8)
            && self.feed_token_max_bytes >= 1
            // `TraceSink` が登録されていない（縦切りには `TraceSink` がない）。
            && !self.trace_enabled;
        if !fields_ok {
            return Err(ERR_INVALID_ARGUMENT);
        }
        let Some(strategy_end) = self
            .strategy_arena_offset
            .checked_add(self.strategy_arena_size)
        else {
            return Err(ERR_CAPACITY_EXCEEDED);
        };
        if strategy_end > self.arena_size {
            return Err(ERR_INVALID_ARGUMENT);
        }
        if stream_count > MAX_STREAMS {
            return Err(ERR_CAPACITY_EXCEEDED);
        }
        let required = self
            .required_bytes(stream_count)
            .ok_or(ERR_CAPACITY_EXCEEDED)?;
        if required > self.max_total_bytes {
            return Err(ERR_CAPACITY_EXCEEDED);
        }
        Ok(required)
    }

    /// 確保量（EK-DD-001 2.3節）。スナップショットの最大サイズ（EK-DS-001 2.3節）と、
    /// State Arena、履歴リング、マージキュー、アクションキューのサイズの合計。
    fn required_bytes(&self, stream_count: u32) -> Option<u64> {
        let record = 32u64.checked_add(u64::from(self.payload_record_size))?;
        let streams = u64::from(stream_count);
        let queue = u64::from(self.action_queue_capacity);
        let history = u64::from(self.history_ring_len);
        let token = round_up8(u64::from(self.feed_token_max_bytes))?;
        let exec_state = round_up8(u64::from(self.exec_state_max_bytes))?;
        let fixed: u64 = 64 + 16 * 9 + 64 + 24 + 40;
        let snapshot = fixed
            .checked_add(self.arena_size)?
            .checked_add(streams.checked_mul(record.checked_add(8)?)?)?
            .checked_add(queue.checked_mul(record)?.checked_add(8)?)?
            .checked_add(history.checked_mul(record)?.checked_add(8)?)?
            .checked_add(
                streams
                    .checked_mul(token.checked_add(16)?)?
                    .checked_add(8)?,
            )?
            .checked_add(exec_state.checked_add(8)?)?;
        let buffers = self
            .arena_size
            .checked_add(history.checked_mul(record)?)?
            .checked_add(streams.checked_mul(record)?)?
            .checked_add(queue.checked_mul(record)?)?;
        snapshot.checked_add(buffers)
    }
}

fn round_up8(value: u64) -> Option<u64> {
    Some(value.checked_add(7)? & !7)
}
