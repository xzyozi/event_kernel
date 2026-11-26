//! `core` の安全な Rust API を C ABI として公開する薄い変換層（EK-IF-001 4.1節、ADR-003）。
//!
//! 全 `extern "C"` 関数で `std::panic::catch_unwind` を適用し、Rust パニックを
//! `EK_ERR_PANIC`（`-99`）へ変換してハンドルを `Poisoned` に遷移させる。

#![allow(unsafe_code)]

use core::ffi::c_void;
use core::mem::{align_of, offset_of, size_of};
use core::slice;
use std::panic::{AssertUnwindSafe, catch_unwind};

use ek_core::{
    Action, EventFeedPort, EventHeader, EventRecord, ExecutionPort, FeedPosition, HistoryView,
    Kernel, KernelConfig, PortError, Prng, RunLimit, SnapshotPort, StepResult, Strategy,
    StrategyStatus, TraceHeader, TraceRecord, TraceSink,
};

/// C ABI メジャーバージョン。
pub const EK_CAPI_ABI_MAJOR: u32 = 0;
/// C ABI マイナーバージョン。
pub const EK_CAPI_ABI_MINOR: u32 = 1;

/// 正常終了。
pub const EK_OK: i32 = 0;
/// 入力が尽きた（正常）。
pub const EK_STEP_FINISHED: i32 = 1;
/// `max_events` 到達（正常）。
pub const EK_RUN_LIMIT_REACHED: i32 = 2;
/// `SnapshotTrigger` 検出（正常）。
pub const EK_RUN_SNAPSHOT_REQUESTED: i32 = 3;

/// 引数制約違反。
pub const EK_ERR_INVALID_ARGUMENT: i32 = -1;
/// 容量上限超過。
pub const EK_ERR_CAPACITY_EXCEEDED: i32 = -2;
/// アクションキュー満杯。
pub const EK_ERR_QUEUE_FULL: i32 = -3;
/// 因果律違反。
pub const EK_ERR_CAUSALITY_VIOLATION: i32 = -4;
/// 同一ストリーム内の連番違反。
pub const EK_ERR_SEQUENCE_VIOLATION: i32 = -5;
/// `Poisoned` ハンドルへの禁止操作。
pub const EK_ERR_POISONED: i32 = -6;
/// コールバック実行中の再入操作。
pub const EK_ERR_REENTRANT: i32 = -7;
/// 非 seekable フィードまたは最大長超過。
pub const EK_ERR_SNAPSHOT_UNSUPPORTED: i32 = -8;
/// スナップショット形式検査失敗。
pub const EK_ERR_SNAPSHOT_INVALID: i32 = -9;
/// 設定または外部ポート検証失敗。
pub const EK_ERR_SNAPSHOT_MISMATCH: i32 = -10;
/// 外部ポート失敗。
pub const EK_ERR_PORT_FAILURE: i32 = -11;
/// ポート結果のキュー超過。
pub const EK_ERR_RESULT_OVERFLOW: i32 = -12;
/// 戦略失敗。
pub const EK_ERR_STRATEGY_FAILURE: i32 = -13;
/// スナップショット I/O 失敗。
pub const EK_ERR_SNAPSHOT_IO: i32 = -14;
/// 境界でのパニック捕捉。
pub const EK_ERR_PANIC: i32 = -99;

const _: () = {
    assert!(EK_OK == ek_core::OK);
    assert!(EK_STEP_FINISHED == ek_core::STEP_FINISHED);
    assert!(EK_RUN_LIMIT_REACHED == ek_core::RUN_LIMIT_REACHED);
    assert!(EK_RUN_SNAPSHOT_REQUESTED == ek_core::RUN_SNAPSHOT_REQUESTED);
    assert!(EK_ERR_INVALID_ARGUMENT == ek_core::ERR_INVALID_ARGUMENT);
    assert!(EK_ERR_CAPACITY_EXCEEDED == ek_core::ERR_CAPACITY_EXCEEDED);
    assert!(EK_ERR_QUEUE_FULL == ek_core::ERR_QUEUE_FULL);
    assert!(EK_ERR_CAUSALITY_VIOLATION == ek_core::ERR_CAUSALITY_VIOLATION);
    assert!(EK_ERR_SEQUENCE_VIOLATION == ek_core::ERR_SEQUENCE_VIOLATION);
    assert!(EK_ERR_POISONED == ek_core::ERR_POISONED);
    assert!(EK_ERR_REENTRANT == ek_core::ERR_REENTRANT);
    assert!(EK_ERR_SNAPSHOT_UNSUPPORTED == ek_core::ERR_SNAPSHOT_UNSUPPORTED);
    assert!(EK_ERR_SNAPSHOT_INVALID == ek_core::ERR_SNAPSHOT_INVALID);
    assert!(EK_ERR_SNAPSHOT_MISMATCH == ek_core::ERR_SNAPSHOT_MISMATCH);
    assert!(EK_ERR_PORT_FAILURE == ek_core::ERR_PORT_FAILURE);
    assert!(EK_ERR_RESULT_OVERFLOW == ek_core::ERR_RESULT_OVERFLOW);
    assert!(EK_ERR_STRATEGY_FAILURE == ek_core::ERR_STRATEGY_FAILURE);
    assert!(EK_ERR_SNAPSHOT_IO == ek_core::ERR_SNAPSHOT_IO);
    assert!(EK_ERR_PANIC == ek_core::ERR_PANIC);
};

/// C ABI 用カーネル設定（EK-IF-001 4.1.2節）。
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EkKernelConfig {
    /// `sizeof(EkKernelConfig)`（ABI 互換性検査用）。
    pub struct_size: u32,
    /// 履歴リング長（1以上）。
    pub history_ring_len: u32,
    /// State Arena 全体サイズ（8の倍数）。
    pub arena_size: u64,
    /// `StrategyArena` オフセット（8の倍数）。
    pub strategy_arena_offset: u64,
    /// `StrategyArena` サイズ（8の倍数）。
    pub strategy_arena_size: u64,
    /// アクションキュー容量（1以上）。
    pub action_queue_capacity: u32,
    /// ペイロードレコード長（8の倍数かつ16以上）。
    pub payload_record_size: u32,
    /// 価格小数スケール指数（0以下）。
    pub price_scale_exp: i32,
    /// 数量小数スケール指数（0以下）。
    pub qty_scale_exp: i32,
    /// PRNG シード。
    pub prng_seed: u64,
    /// `EXEC_STATE` 最大バイト数（8の倍数）。
    pub exec_state_max_bytes: u32,
    /// 位置トークン最大バイト数（1以上）。
    pub feed_token_max_bytes: u32,
    /// 状態ハッシュ計算間隔（0は無効）。
    pub hash_interval: u64,
    /// トレース出力有効フラグ（0または1）。
    pub trace_enabled: u8,
    /// 予約領域（0埋め必須）。
    pub reserved: [u8; 7],
    /// `create` が確保する合計バイト数の上限。
    pub max_total_bytes: u64,
}

/// C ABI 用イベントヘッダ（32バイト、8バイト境界）。
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EkEventHeader {
    /// 論理発生時刻（ナノ秒）。
    pub timestamp_ns: i64,
    /// ストリーム内連番。
    pub sequence_id: u64,
    /// ストリーム識別子。
    pub stream_id: u32,
    /// イベント種別。
    pub event_type: u16,
    /// 制御フラグ。
    pub flags: u16,
    /// 入力側 SoA 配列添字（内部・保存時は0扱い）。
    pub payload_idx: u64,
}

const _: () = {
    assert!(size_of::<EkEventHeader>() == 32);
    assert!(align_of::<EkEventHeader>() == 8);
    assert!(offset_of!(EkEventHeader, timestamp_ns) == 0);
    assert!(offset_of!(EkEventHeader, sequence_id) == 8);
    assert!(offset_of!(EkEventHeader, stream_id) == 16);
    assert!(offset_of!(EkEventHeader, event_type) == 20);
    assert!(offset_of!(EkEventHeader, flags) == 22);
    assert!(offset_of!(EkEventHeader, payload_idx) == 24);
};

impl From<EventHeader> for EkEventHeader {
    #[inline]
    fn from(h: EventHeader) -> Self {
        Self {
            timestamp_ns: h.timestamp_ns,
            sequence_id: h.sequence_id,
            stream_id: h.stream_id,
            event_type: h.event_type,
            flags: h.flags,
            payload_idx: h.payload_idx,
        }
    }
}

impl From<EkEventHeader> for EventHeader {
    #[inline]
    fn from(h: EkEventHeader) -> Self {
        Self {
            timestamp_ns: h.timestamp_ns,
            sequence_id: h.sequence_id,
            stream_id: h.stream_id,
            event_type: h.event_type,
            flags: h.flags,
            payload_idx: h.payload_idx,
        }
    }
}

/// C ABI 用ステップ・ラン結果（EK-IF-001 4.1.2節）。
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EkStepResult {
    /// 終了コードまたは負のエラーコード。
    pub status: i32,
    /// 戦略またはポートが返した詳細コード。
    pub detail: i32,
    /// 累計処理イベント数。
    pub processed_event_count: u64,
    /// 現在の論理時刻。
    pub now_ns: i64,
}

impl From<StepResult> for EkStepResult {
    #[inline]
    fn from(r: StepResult) -> Self {
        Self {
            status: r.status,
            detail: r.detail,
            processed_event_count: r.processed_event_count,
            now_ns: r.now_ns,
        }
    }
}

/// C ABI 用遅延アクション（EK-IF-001 4.1.2節）。
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct EkAction {
    /// 戦略・`ExecutionPort` 間の識別子。
    pub action_kind: u16,
    /// 予約領域（0埋め必須）。
    pub reserved: [u8; 6],
    /// 到達までの遅延（1以上）。
    pub latency_ns: i64,
    /// ペイロードポインタ（`payload_len > 0` のとき非 NULL）。
    pub payload_ptr: *const u8,
    /// ペイロード長（`payload_record_size - 8` 以下）。
    pub payload_len: u32,
}

/// コールバック内から PRNG を参照するための不透明型。
pub struct EkPrng {
    ptr: *mut Prng,
}

/// 戦略コールバック内から `submit_action` を呼ぶための不透明コンテキスト。
pub struct EkActionContext {
    submit_fn: *mut dyn FnMut(&Action<'_>) -> i32,
}

/// 戦略コールバックへ渡されるコンテキスト（EK-IF-001 4.1.2節）。
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct EkStrategyCtx {
    /// 現在の論理時刻。
    pub now_ns: i64,
    /// 確定済み過去イベントヘッダ配列の先頭（コールバック中のみ有効）。
    pub history_headers: *const EkEventHeader,
    /// 有効な履歴件数。
    pub history_len: u32,
    /// `StrategyArena` 先頭ポインタ（コールバック中のみ有効）。
    pub strategy_arena_ptr: *mut u8,
    /// `StrategyArena` バイト長。
    pub strategy_arena_len: u64,
    /// 共有決定論 PRNG ハンドル。
    pub prng: *mut EkPrng,
    /// アクション提出用不透明コンテキスト。
    pub action_ctx: *mut EkActionContext,
}

/// 外部フィード vtable（EK-IF-001 4.1.2節）。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct EkFeedVTable {
    /// コールバック側ユーザーデータ。
    pub user_data: *mut c_void,
    /// 次イベント時刻取得。`out_has_next=0` で終端。戻り値は0で成功、負値で失敗。
    pub peek_next_timestamp: Option<
        unsafe extern "C-unwind" fn(
            user_data: *mut c_void,
            out_has_next: *mut u8,
            out_timestamp_ns: *mut i64,
        ) -> i32,
    >,
    /// 次イベント取り出し。戻り値は0で成功、負値で失敗。
    pub pop_next: Option<
        unsafe extern "C-unwind" fn(
            user_data: *mut c_void,
            out_header: *mut EkEventHeader,
            out_payload: *mut u8,
            payload_cap: usize,
        ) -> i32,
    >,
    /// 読み取り位置取得（任意: NULL なら非 seekable 扱い）。
    pub position: Option<
        unsafe extern "C-unwind" fn(
            user_data: *mut c_void,
            out_seekable: *mut u8,
            out_token: *mut u8,
            token_cap: usize,
            out_token_len: *mut usize,
        ) -> i32,
    >,
    /// 復元検証フェーズ用位置トークン検査（任意: NULL なら成功扱い）。
    pub validate_position: Option<
        unsafe extern "C-unwind" fn(
            user_data: *mut c_void,
            token_ptr: *const u8,
            token_len: usize,
        ) -> i32,
    >,
    /// 復元適用フェーズ用位置巻き戻し（任意: NULL なら失敗）。
    pub seek: Option<
        unsafe extern "C-unwind" fn(
            user_data: *mut c_void,
            token_ptr: *const u8,
            token_len: usize,
        ) -> i32,
    >,
}

/// 約定ポート vtable（EK-IF-001 4.1.2節）。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct EkExecVTable {
    /// コールバック側ユーザーデータ。
    pub user_data: *mut c_void,
    /// `ActionArrival` 到達通知。
    pub on_arrival: Option<
        unsafe extern "C-unwind" fn(
            user_data: *mut c_void,
            header: *const EkEventHeader,
            payload: *const u8,
            payload_len: usize,
            prng: *mut EkPrng,
        ) -> i32,
    >,
    /// 市場イベント通知。
    pub on_market_event: Option<
        unsafe extern "C-unwind" fn(
            user_data: *mut c_void,
            header: *const EkEventHeader,
            payload: *const u8,
            payload_len: usize,
            history_headers: *const EkEventHeader,
            history_len: u32,
            prng: *mut EkPrng,
        ) -> i32,
    >,
    /// 約定結果を1件取り出す。`out_has_result=0` で終了。
    pub poll_result: Option<
        unsafe extern "C-unwind" fn(
            user_data: *mut c_void,
            out_has_result: *mut u8,
            out_header: *mut EkEventHeader,
            out_payload: *mut u8,
            payload_cap: usize,
        ) -> i32,
    >,
    /// 内部状態シリアライズ（任意: NULL なら0バイト）。
    pub capture_state: Option<
        unsafe extern "C-unwind" fn(
            user_data: *mut c_void,
            out_state: *mut u8,
            state_cap: usize,
            out_len: *mut usize,
        ) -> i32,
    >,
    /// 復元検証フェーズ用状態検査（任意: NULL なら成功）。
    pub validate_state: Option<
        unsafe extern "C-unwind" fn(
            user_data: *mut c_void,
            state_ptr: *const u8,
            state_len: usize,
        ) -> i32,
    >,
    /// 復元適用フェーズ用状態上書き（任意: NULL なら成功）。
    pub restore_state: Option<
        unsafe extern "C-unwind" fn(
            user_data: *mut c_void,
            state_ptr: *const u8,
            state_len: usize,
        ) -> i32,
    >,
}

/// スナップショット保存ポート vtable（EK-IF-001 4.1.2節）。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct EkSnapVTable {
    /// コールバック側ユーザーデータ。
    pub user_data: *mut c_void,
    /// スナップショットバイト列を保存する。
    pub store: Option<
        unsafe extern "C-unwind" fn(
            user_data: *mut c_void,
            blob_ptr: *const u8,
            blob_len: usize,
            logical_time_ns: i64,
            event_count: u64,
        ) -> i32,
    >,
}

/// トレース出力先 vtable（EK-IF-001 4.1.2節）。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct EkTraceVTable {
    /// コールバック側ユーザーデータ。
    pub user_data: *mut c_void,
    /// 初期化時の32バイト `TRACE_HEADER` 書き込み。
    pub write_header:
        Option<unsafe extern "C-unwind" fn(user_data: *mut c_void, header_bytes: *const u8) -> i32>,
    /// ステップ完了後の40バイト `TRACE_RECORD` 書き込み。
    pub write:
        Option<unsafe extern "C-unwind" fn(user_data: *mut c_void, record_bytes: *const u8) -> i32>,
}

/// 戦略コールバック vtable（EK-IF-001 4.1.2節）。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct EkStrategyVTable {
    /// コールバック側ユーザーデータ。
    pub user_data: *mut c_void,
    /// イベント配送コールバック。0は正常、1以上は戦略異常、負値は境界エラーコード相当。
    pub on_event: Option<
        unsafe extern "C-unwind" fn(
            user_data: *mut c_void,
            ctx: *const EkStrategyCtx,
            header: *const EkEventHeader,
            payload: *const u8,
            payload_len: usize,
        ) -> u32,
    >,
}

struct CFeedPort {
    vtable: EkFeedVTable,
}

impl EventFeedPort for CFeedPort {
    fn peek_next_timestamp(&mut self) -> Result<Option<i64>, PortError> {
        let Some(f) = self.vtable.peek_next_timestamp else {
            return Err(PortError(EK_ERR_INVALID_ARGUMENT));
        };
        let mut has_next: u8 = 0;
        let mut ts: i64 = 0;
        let rc = unsafe { f(self.vtable.user_data, &mut has_next, &mut ts) };
        if rc != 0 {
            return Err(PortError(rc));
        }
        Ok(if has_next != 0 { Some(ts) } else { None })
    }

    fn pop_next(&mut self, header: &mut EventHeader, payload: &mut [u8]) -> Result<(), PortError> {
        let Some(f) = self.vtable.pop_next else {
            return Err(PortError(EK_ERR_INVALID_ARGUMENT));
        };
        let mut c_header = EkEventHeader::default();
        let rc = unsafe {
            f(
                self.vtable.user_data,
                &mut c_header,
                payload.as_mut_ptr(),
                payload.len(),
            )
        };
        if rc != 0 {
            return Err(PortError(rc));
        }
        *header = EventHeader::from(c_header);
        Ok(())
    }

    fn position(&self, out: &mut [u8]) -> Result<FeedPosition, PortError> {
        let Some(f) = self.vtable.position else {
            return Ok(FeedPosition {
                seekable: false,
                len: 0,
            });
        };
        let mut seekable: u8 = 0;
        let mut len: usize = 0;
        let rc = unsafe {
            f(
                self.vtable.user_data,
                &mut seekable,
                out.as_mut_ptr(),
                out.len(),
                &mut len,
            )
        };
        if rc != 0 {
            return Err(PortError(rc));
        }
        Ok(FeedPosition {
            seekable: seekable != 0,
            len,
        })
    }

    fn validate_position(&self, token: &[u8]) -> Result<(), PortError> {
        let Some(f) = self.vtable.validate_position else {
            return Ok(());
        };
        let rc = unsafe { f(self.vtable.user_data, token.as_ptr(), token.len()) };
        if rc == 0 { Ok(()) } else { Err(PortError(rc)) }
    }

    fn seek(&mut self, token: &[u8]) -> Result<(), PortError> {
        let Some(f) = self.vtable.seek else {
            return Err(PortError(0));
        };
        let rc = unsafe { f(self.vtable.user_data, token.as_ptr(), token.len()) };
        if rc == 0 { Ok(()) } else { Err(PortError(rc)) }
    }
}

struct CExecPort {
    vtable: EkExecVTable,
    history_scratch: Vec<EkEventHeader>,
}

fn fill_history_scratch(scratch: &mut Vec<EkEventHeader>, history: HistoryView<'_>) -> u32 {
    scratch.clear();
    let count = history.len().min(scratch.capacity());
    for i in 0..count {
        if let Some(rec) = history.get(i) {
            scratch.push(EkEventHeader::from(rec.header));
        }
    }
    u32::try_from(scratch.len()).unwrap_or(u32::MAX)
}

impl ExecutionPort for CExecPort {
    fn on_arrival(&mut self, event: EventRecord<'_>, rng: &mut Prng) -> Result<(), PortError> {
        let Some(f) = self.vtable.on_arrival else {
            return Err(PortError(EK_ERR_INVALID_ARGUMENT));
        };
        let header = EkEventHeader::from(event.header);
        let mut c_prng = EkPrng { ptr: rng };
        let rc = unsafe {
            f(
                self.vtable.user_data,
                &header,
                event.payload.as_ptr(),
                event.payload.len(),
                &mut c_prng,
            )
        };
        if rc == 0 { Ok(()) } else { Err(PortError(rc)) }
    }

    fn on_market_event(
        &mut self,
        event: EventRecord<'_>,
        history: HistoryView<'_>,
        rng: &mut Prng,
    ) -> Result<(), PortError> {
        let Some(f) = self.vtable.on_market_event else {
            return Err(PortError(EK_ERR_INVALID_ARGUMENT));
        };
        let header = EkEventHeader::from(event.header);
        let history_len = fill_history_scratch(&mut self.history_scratch, history);
        let history_ptr = if history_len == 0 {
            core::ptr::null()
        } else {
            self.history_scratch.as_ptr()
        };
        let mut c_prng = EkPrng { ptr: rng };
        let rc = unsafe {
            f(
                self.vtable.user_data,
                &header,
                event.payload.as_ptr(),
                event.payload.len(),
                history_ptr,
                history_len,
                &mut c_prng,
            )
        };
        if rc == 0 { Ok(()) } else { Err(PortError(rc)) }
    }

    fn poll_result(
        &mut self,
        header: &mut EventHeader,
        payload: &mut [u8],
    ) -> Result<bool, PortError> {
        let Some(f) = self.vtable.poll_result else {
            return Err(PortError(EK_ERR_INVALID_ARGUMENT));
        };
        let mut has_result: u8 = 0;
        let mut c_header = EkEventHeader::default();
        let rc = unsafe {
            f(
                self.vtable.user_data,
                &mut has_result,
                &mut c_header,
                payload.as_mut_ptr(),
                payload.len(),
            )
        };
        if rc != 0 {
            return Err(PortError(rc));
        }
        if has_result == 0 {
            return Ok(false);
        }
        *header = EventHeader::from(c_header);
        Ok(true)
    }

    fn capture_state(&self, out: &mut [u8]) -> Result<usize, PortError> {
        let Some(f) = self.vtable.capture_state else {
            return Ok(0);
        };
        let mut out_len: usize = 0;
        let rc = unsafe {
            f(
                self.vtable.user_data,
                out.as_mut_ptr(),
                out.len(),
                &mut out_len,
            )
        };
        if rc == 0 {
            Ok(out_len)
        } else {
            Err(PortError(rc))
        }
    }

    fn validate_state(&self, state: &[u8]) -> Result<(), PortError> {
        let Some(f) = self.vtable.validate_state else {
            return Ok(());
        };
        let rc = unsafe { f(self.vtable.user_data, state.as_ptr(), state.len()) };
        if rc == 0 { Ok(()) } else { Err(PortError(rc)) }
    }

    fn restore_state(&mut self, state: &[u8]) -> Result<(), PortError> {
        let Some(f) = self.vtable.restore_state else {
            return Ok(());
        };
        let rc = unsafe { f(self.vtable.user_data, state.as_ptr(), state.len()) };
        if rc == 0 { Ok(()) } else { Err(PortError(rc)) }
    }
}

struct CSnapPort {
    vtable: EkSnapVTable,
}

impl SnapshotPort for CSnapPort {
    fn store(
        &mut self,
        blob: &[u8],
        logical_time_ns: i64,
        event_count: u64,
    ) -> Result<(), PortError> {
        let Some(f) = self.vtable.store else {
            return Err(PortError(EK_ERR_INVALID_ARGUMENT));
        };
        let rc = unsafe {
            f(
                self.vtable.user_data,
                blob.as_ptr(),
                blob.len(),
                logical_time_ns,
                event_count,
            )
        };
        if rc == 0 { Ok(()) } else { Err(PortError(rc)) }
    }
}

struct CTraceSink {
    vtable: EkTraceVTable,
}

impl TraceSink for CTraceSink {
    fn write_header(&mut self, header: &TraceHeader) -> Result<(), PortError> {
        let Some(f) = self.vtable.write_header else {
            return Err(PortError(EK_ERR_INVALID_ARGUMENT));
        };
        let bytes = header.to_le_bytes();
        let rc = unsafe { f(self.vtable.user_data, bytes.as_ptr()) };
        if rc == 0 { Ok(()) } else { Err(PortError(rc)) }
    }

    fn write(&mut self, record: &TraceRecord) -> Result<(), PortError> {
        let Some(f) = self.vtable.write else {
            return Err(PortError(EK_ERR_INVALID_ARGUMENT));
        };
        let bytes = record.to_le_bytes();
        let rc = unsafe { f(self.vtable.user_data, bytes.as_ptr()) };
        if rc == 0 { Ok(()) } else { Err(PortError(rc)) }
    }
}

struct CStrategy {
    vtable: EkStrategyVTable,
    history_scratch: Vec<EkEventHeader>,
}

impl Strategy for CStrategy {
    fn on_event(
        &mut self,
        context: &mut ek_core::Context<'_>,
        event: EventRecord<'_>,
    ) -> StrategyStatus {
        let Some(f) = self.vtable.on_event else {
            return 1;
        };
        let now_ns = context.now_ns();
        let history_len = fill_history_scratch(&mut self.history_scratch, context.history());
        let history_headers = if history_len == 0 {
            core::ptr::null()
        } else {
            self.history_scratch.as_ptr()
        };

        let header = EkEventHeader::from(event.header);
        let arena_len = context.strategy_arena().len() as u64;
        let arena_ptr = if arena_len == 0 {
            core::ptr::null_mut()
        } else {
            context.strategy_arena().as_mut_ptr()
        };

        // 戦略は `Context` の PRNG と submit_action を共有しないため、
        // コールバック中は action 提出と PRNG をそれぞれポインタ経由で委譲する。
        let mut prng_copy = context.rng().clone();
        let mut c_prng = EkPrng {
            ptr: &mut prng_copy,
        };
        let mut submit_closure = |action: &Action<'_>| context.submit_action(action);
        // 安全注記: `action_ctx` と `c_prng` はコールバック終了時に直ちに無効となり外へ持ち出せない。
        let submit_dyn: &mut dyn FnMut(&Action<'_>) -> i32 = &mut submit_closure;
        let submit_ptr: *mut dyn FnMut(&Action<'_>) -> i32 = unsafe {
            core::mem::transmute::<
                *mut dyn FnMut(&Action<'_>) -> i32,
                *mut (dyn FnMut(&Action<'_>) -> i32 + 'static),
            >(submit_dyn)
        };
        let mut action_ctx = EkActionContext {
            submit_fn: submit_ptr,
        };

        let ctx = EkStrategyCtx {
            now_ns,
            history_headers,
            history_len,
            strategy_arena_ptr: arena_ptr,
            strategy_arena_len: arena_len,
            prng: &mut c_prng,
            action_ctx: &mut action_ctx,
        };
        let status = unsafe {
            f(
                self.vtable.user_data,
                &ctx,
                &header,
                event.payload.as_ptr(),
                event.payload.len(),
            )
        };
        *context.rng() = prng_copy;
        status
    }
}

/// C ABI 用カーネル不透明ハンドル。
pub struct EkKernelHandle {
    kernel: Kernel<CFeedPort, CExecPort, CStrategy, Option<CTraceSink>>,
}

fn validate_config(cfg: &EkKernelConfig) -> Result<KernelConfig, i32> {
    if cfg.struct_size as usize != size_of::<EkKernelConfig>()
        || cfg.reserved != [0u8; 7]
        || cfg.trace_enabled > 1
    {
        return Err(EK_ERR_INVALID_ARGUMENT);
    }
    Ok(KernelConfig {
        history_ring_len: cfg.history_ring_len,
        arena_size: cfg.arena_size,
        strategy_arena_offset: cfg.strategy_arena_offset,
        strategy_arena_size: cfg.strategy_arena_size,
        action_queue_capacity: cfg.action_queue_capacity,
        payload_record_size: cfg.payload_record_size,
        price_scale_exp: cfg.price_scale_exp,
        qty_scale_exp: cfg.qty_scale_exp,
        prng_seed: cfg.prng_seed,
        exec_state_max_bytes: cfg.exec_state_max_bytes,
        feed_token_max_bytes: cfg.feed_token_max_bytes,
        hash_interval: cfg.hash_interval,
        trace_enabled: cfg.trace_enabled == 1,
        max_total_bytes: cfg.max_total_bytes,
    })
}

fn catch_handle_op<F>(handle: *mut EkKernelHandle, out_result: *mut EkStepResult, op: F) -> i32
where
    F: FnOnce(&EkKernelHandle) -> i32,
{
    if handle.is_null() {
        return EK_ERR_INVALID_ARGUMENT;
    }
    match catch_unwind(AssertUnwindSafe(|| {
        let h = unsafe { &*handle };
        op(h)
    })) {
        Ok(rc) => rc,
        Err(_) => {
            unsafe {
                (*handle).kernel.poison();
                if !out_result.is_null() {
                    (*out_result) = EkStepResult {
                        status: EK_ERR_PANIC,
                        detail: 0,
                        processed_event_count: 0,
                        now_ns: 0,
                    };
                }
            }
            EK_ERR_PANIC
        }
    }
}

/// C ABI バージョンを取得する（EK-IF-001 4.1.1節）。
///
/// # Safety
/// `out_major` と `out_minor` は非 NULL かつ書き込み可能な有効ポインタであること。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ek_capi_abi_version(out_major: *mut u32, out_minor: *mut u32) -> i32 {
    if out_major.is_null() || out_minor.is_null() {
        return EK_ERR_INVALID_ARGUMENT;
    }
    unsafe {
        *out_major = EK_CAPI_ABI_MAJOR;
        *out_minor = EK_CAPI_ABI_MINOR;
    }
    EK_OK
}

/// 共有決定論 PRNG から次の 64 ビット乱数を取得する。
///
/// # Safety
/// `prng` はコールバック中に渡された有効なポインタ、`out_value` は有効な書き込み先であること。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ek_prng_next_u64(prng: *mut EkPrng, out_value: *mut u64) -> i32 {
    if prng.is_null() || out_value.is_null() {
        return EK_ERR_INVALID_ARGUMENT;
    }
    match catch_unwind(AssertUnwindSafe(|| unsafe {
        let inner = &mut *(*prng).ptr;
        *out_value = inner.next_u64();
        EK_OK
    })) {
        Ok(rc) => rc,
        Err(_) => EK_ERR_PANIC,
    }
}

/// 検証と初期メモリ確保を行い、`Ready` 状態のハンドルを作成する（F-001）。
///
/// # Safety
/// 各ポインタ引数は有効で、対応する `user_data` および vtable 関数群はハンドル破棄まで有効であること。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ek_kernel_create(
    config: *const EkKernelConfig,
    feeds: *const EkFeedVTable,
    feed_count: usize,
    exec_vtable: *const EkExecVTable,
    trace_vtable: *const EkTraceVTable,
    strategy_vtable: *const EkStrategyVTable,
    out_handle: *mut *mut EkKernelHandle,
) -> i32 {
    if config.is_null()
        || feeds.is_null()
        || exec_vtable.is_null()
        || strategy_vtable.is_null()
        || out_handle.is_null()
        || feed_count == 0
    {
        return EK_ERR_INVALID_ARGUMENT;
    }
    unsafe {
        *out_handle = core::ptr::null_mut();
    }

    match catch_unwind(AssertUnwindSafe(|| {
        let cfg = unsafe { &*config };
        let core_cfg = match validate_config(cfg) {
            Ok(c) => c,
            Err(code) => return code,
        };
        let feed_vtables = unsafe { slice::from_raw_parts(feeds, feed_count) };
        let mut feed_ports = Vec::with_capacity(feed_count);
        for v in feed_vtables {
            if v.peek_next_timestamp.is_none() || v.pop_next.is_none() {
                return EK_ERR_INVALID_ARGUMENT;
            }
            feed_ports.push(CFeedPort { vtable: *v });
        }
        let exec_v = unsafe { *exec_vtable };
        if exec_v.on_arrival.is_none()
            || exec_v.on_market_event.is_none()
            || exec_v.poll_result.is_none()
        {
            return EK_ERR_INVALID_ARGUMENT;
        }
        let strat_v = unsafe { *strategy_vtable };
        if strat_v.on_event.is_none() {
            return EK_ERR_INVALID_ARGUMENT;
        }
        let trace_sink = if trace_vtable.is_null() {
            None
        } else {
            let t_v = unsafe { *trace_vtable };
            if core_cfg.trace_enabled && (t_v.write_header.is_none() || t_v.write.is_none()) {
                return EK_ERR_INVALID_ARGUMENT;
            }
            Some(CTraceSink { vtable: t_v })
        };

        let hist_cap = usize::try_from(core_cfg.history_ring_len).unwrap_or(0);
        let exec_port = CExecPort {
            vtable: exec_v,
            history_scratch: Vec::with_capacity(hist_cap),
        };
        let strategy = CStrategy {
            vtable: strat_v,
            history_scratch: Vec::with_capacity(hist_cap),
        };

        match Kernel::create_multi_with_trace(core_cfg, feed_ports, exec_port, strategy, trace_sink)
        {
            Ok(kernel) => {
                let boxed = Box::new(EkKernelHandle { kernel });
                unsafe {
                    *out_handle = Box::into_raw(boxed);
                }
                EK_OK
            }
            Err(code) => code,
        }
    })) {
        Ok(rc) => rc,
        Err(_) => EK_ERR_PANIC,
    }
}

/// 1イベントを処理する（F-002）。
///
/// # Safety
/// `handle` は `ek_kernel_create` で作成した有効なハンドル、`out_result` は有効な書き込み先であること。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ek_kernel_step(
    handle: *mut EkKernelHandle,
    out_result: *mut EkStepResult,
) -> i32 {
    if out_result.is_null() {
        return EK_ERR_INVALID_ARGUMENT;
    }
    catch_handle_op(handle, out_result, |h| {
        let r = EkStepResult::from(h.kernel.step());
        unsafe {
            *out_result = r;
        }
        r.status
    })
}

/// 停止条件まで `step` を繰り返す（F-003、`max_events = 0` は無制限）。
///
/// # Safety
/// `handle` は有効なハンドル、`out_result` は有効な書き込み先であること。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ek_kernel_run(
    handle: *mut EkKernelHandle,
    max_events: u64,
    out_result: *mut EkStepResult,
) -> i32 {
    if out_result.is_null() {
        return EK_ERR_INVALID_ARGUMENT;
    }
    catch_handle_op(handle, out_result, |h| {
        let r = EkStepResult::from(h.kernel.run(RunLimit { max_events }));
        unsafe {
            *out_result = r;
        }
        r.status
    })
}

/// 戦略コールバック内から遅延アクションを提出する（F-004）。
///
/// # Safety
/// `ctx` は進行中の戦略コールバックへ渡されたポインタ、`action` および非0長時の `payload_ptr` は有効であること。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ek_kernel_submit_action(
    ctx: *mut EkActionContext,
    action: *const EkAction,
) -> i32 {
    if ctx.is_null() || action.is_null() {
        return EK_ERR_INVALID_ARGUMENT;
    }
    match catch_unwind(AssertUnwindSafe(|| {
        let a = unsafe { &*action };
        if a.reserved != [0u8; 6] {
            return EK_ERR_INVALID_ARGUMENT;
        }
        let payload = if a.payload_len == 0 {
            &[][..]
        } else {
            if a.payload_ptr.is_null() {
                return EK_ERR_INVALID_ARGUMENT;
            }
            unsafe { slice::from_raw_parts(a.payload_ptr, a.payload_len as usize) }
        };
        let core_action = Action {
            action_kind: a.action_kind,
            latency_ns: a.latency_ns,
            payload,
        };
        let submit = unsafe { &mut *(*ctx).submit_fn };
        submit(&core_action)
    })) {
        Ok(rc) => rc,
        Err(_) => EK_ERR_PANIC,
    }
}

/// 現在の完全状態を `SnapshotPort` 経由で保存する（F-005）。
///
/// # Safety
/// `handle` は有効なハンドル、`snap_vtable` は有効な vtable を指していること。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ek_kernel_capture(
    handle: *mut EkKernelHandle,
    snap_vtable: *const EkSnapVTable,
) -> i32 {
    if snap_vtable.is_null() {
        return EK_ERR_INVALID_ARGUMENT;
    }
    catch_handle_op(handle, core::ptr::null_mut(), |h| {
        let v = unsafe { *snap_vtable };
        if v.store.is_none() {
            return EK_ERR_INVALID_ARGUMENT;
        }
        let mut port = CSnapPort { vtable: v };
        h.kernel.capture(&mut port)
    })
}

/// スナップショットバイト列から状態を復元する（F-006）。
///
/// # Safety
/// `handle` は有効なハンドル、`blob` は `blob_len` バイトの読み取り可能な有効領域であること。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ek_kernel_restore(
    handle: *mut EkKernelHandle,
    blob: *const u8,
    blob_len: usize,
) -> i32 {
    if blob.is_null() && blob_len > 0 {
        return EK_ERR_INVALID_ARGUMENT;
    }
    catch_handle_op(handle, core::ptr::null_mut(), |h| {
        let slice_bytes = if blob_len == 0 {
            &[][..]
        } else {
            unsafe { slice::from_raw_parts(blob, blob_len) }
        };
        h.kernel.restore(slice_bytes)
    })
}

/// 保存を伴わず正準バイト列の全セクション SHA-256 を算出する。
///
/// # Safety
/// `handle` は有効なハンドル、`out_sha256` は 32 バイトの書き込み可能な有効領域であること。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ek_kernel_state_digest(
    handle: *mut EkKernelHandle,
    out_sha256: *mut u8,
) -> i32 {
    if out_sha256.is_null() {
        return EK_ERR_INVALID_ARGUMENT;
    }
    catch_handle_op(handle, core::ptr::null_mut(), |h| {
        match h.kernel.state_digest() {
            Ok(digest) => {
                unsafe {
                    core::ptr::copy_nonoverlapping(digest.as_ptr(), out_sha256, 32);
                }
                EK_OK
            }
            Err(code) => code,
        }
    })
}

/// 実行前に `StrategyArena` へ初期値を書き込む（F-011）。
///
/// # Safety
/// `handle` は有効なハンドル、`bytes` は `len` バイトの読み取り可能な有効領域であること。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ek_kernel_init_strategy_arena(
    handle: *mut EkKernelHandle,
    offset: u64,
    bytes: *const u8,
    len: usize,
) -> i32 {
    if bytes.is_null() && len > 0 {
        return EK_ERR_INVALID_ARGUMENT;
    }
    catch_handle_op(handle, core::ptr::null_mut(), |h| {
        let data = if len == 0 {
            &[][..]
        } else {
            unsafe { slice::from_raw_parts(bytes, len) }
        };
        h.kernel.init_strategy_arena(offset, data)
    })
}

/// `StrategyArena` の内容を呼び出し側バッファへコピーする（F-012）。
///
/// # Safety
/// `handle` は有効なハンドル、`out_bytes` は `len` バイトの書き込み可能な有効領域であること。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ek_kernel_read_strategy_arena(
    handle: *mut EkKernelHandle,
    offset: u64,
    out_bytes: *mut u8,
    len: usize,
) -> i32 {
    if out_bytes.is_null() && len > 0 {
        return EK_ERR_INVALID_ARGUMENT;
    }
    catch_handle_op(handle, core::ptr::null_mut(), |h| {
        let buf = if len == 0 {
            &mut [][..]
        } else {
            unsafe { slice::from_raw_parts_mut(out_bytes, len) }
        };
        h.kernel.read_strategy_arena(offset, buf)
    })
}

/// ハンドルと確保済みメモリを解放する（F-007）。NULL は無処理で `EK_OK`。
///
/// # Safety
/// `handle` は NULL または `ek_kernel_create` が返し未破棄の有効なハンドル（かつコールバック外）であること。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ek_kernel_destroy(handle: *mut EkKernelHandle) -> i32 {
    if handle.is_null() {
        return EK_OK;
    }
    match catch_unwind(AssertUnwindSafe(|| {
        let boxed = unsafe { Box::from_raw(handle) };
        let _ = boxed.kernel.destroy_multi_with_trace();
        EK_OK
    })) {
        Ok(rc) => rc,
        Err(_) => EK_ERR_PANIC,
    }
}
