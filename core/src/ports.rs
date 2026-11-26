//! ポートと戦略の呼び出し契約（EK-DD-001 2.7節、2.8節）。

use alloc::boxed::Box;

use crate::event::{EventHeader, EventRecord};
use crate::history::HistoryView;
use crate::kernel::Context;
use crate::prng::Prng;
use crate::trace::{TraceHeader, TraceRecord};

/// ポートの失敗。値は、ポートが定義する詳細コードで、`StepResult.detail` に入る。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PortError(pub i32);

/// 外部フィードの読み取り位置（EK-DS-001 2.2節 `FEED_STATE_ENTRY`）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FeedPosition {
    /// 位置を戻せるデータ源なら true。false の場合、`token_len` は0として扱う。
    pub seekable: bool,
    /// `out` へ書き込んだ位置トークンのバイト数。
    pub len: usize,
}

/// 外部フィード（F-008）。`seek` はスナップショット復元時に追加予定。
pub trait EventFeedPort {
    /// 次のイベントの時刻を返す。`None` は終端。
    fn peek_next_timestamp(&mut self) -> Result<Option<i64>, PortError>;

    /// 次のイベントを取り出し、ヘッダとペイロードを `core` のバッファへコピーする。
    fn pop_next(&mut self, header: &mut EventHeader, payload: &mut [u8]) -> Result<(), PortError>;

    /// 現在の読み取り位置を `out` へ書き込む。位置を戻せないデータ源は `seekable: false` を返す。
    fn position(&self, _out: &mut [u8]) -> Result<FeedPosition, PortError> {
        Ok(FeedPosition {
            seekable: false,
            len: 0,
        })
    }
}

impl<F: EventFeedPort + ?Sized> EventFeedPort for Box<F> {
    fn peek_next_timestamp(&mut self) -> Result<Option<i64>, PortError> {
        (**self).peek_next_timestamp()
    }

    fn pop_next(&mut self, header: &mut EventHeader, payload: &mut [u8]) -> Result<(), PortError> {
        (**self).pop_next(header, payload)
    }

    fn position(&self, out: &mut [u8]) -> Result<FeedPosition, PortError> {
        (**self).position(out)
    }
}

/// 約定判定（F-009）。`restore_state` と `validate_state` はスナップショット復元時に追加予定。
pub trait ExecutionPort {
    /// `ActionArrival` を配送するときに呼ばれる。
    fn on_arrival(&mut self, event: EventRecord<'_>, rng: &mut Prng) -> Result<(), PortError>;

    /// 市場イベントを配送するときに、履歴への追加と戦略への通知より前に呼ばれる。
    /// `history` は、今のイベントを含まない。
    fn on_market_event(
        &mut self,
        event: EventRecord<'_>,
        history: HistoryView<'_>,
        rng: &mut Prng,
    ) -> Result<(), PortError>;

    /// 積まれた結果を、生成した順に1件ずつ返す。結果がなければ `Ok(false)`。
    fn poll_result(
        &mut self,
        header: &mut EventHeader,
        payload: &mut [u8],
    ) -> Result<bool, PortError>;

    /// 内部状態を決定的なバイト列として `out` へ書き込む。書き込んだバイト数を返す。
    fn capture_state(&self, _out: &mut [u8]) -> Result<usize, PortError> {
        Ok(0)
    }
}

/// トレース出力先（EK-DD-001 2.7節、EK-DS-001 3.1節）。
pub trait TraceSink {
    /// `TraceSink` が登録されているかどうか。`NoTraceSink` と `None` は false。
    fn is_present(&self) -> bool {
        true
    }

    /// 初期化時（`trace_enabled` が true のとき）に `TRACE_HEADER` を書き込む。
    fn write_header(&mut self, header: &TraceHeader) -> Result<(), PortError>;

    /// ステップ完了後に `TRACE_RECORD` を1件出力する。
    fn write(&mut self, record: &TraceRecord) -> Result<(), PortError>;
}

/// `TraceSink` を登録しない `Kernel::create` 用のプレースホルダ。
#[derive(Clone, Copy, Debug, Default)]
pub struct NoTraceSink;

impl TraceSink for NoTraceSink {
    fn is_present(&self) -> bool {
        false
    }

    fn write_header(&mut self, _header: &TraceHeader) -> Result<(), PortError> {
        Err(PortError(0))
    }

    fn write(&mut self, _record: &TraceRecord) -> Result<(), PortError> {
        Err(PortError(0))
    }
}

impl<T: TraceSink> TraceSink for Option<T> {
    fn is_present(&self) -> bool {
        self.as_ref().is_some_and(TraceSink::is_present)
    }

    fn write_header(&mut self, header: &TraceHeader) -> Result<(), PortError> {
        match self {
            Some(inner) => inner.write_header(header),
            None => Err(PortError(0)),
        }
    }

    fn write(&mut self, record: &TraceRecord) -> Result<(), PortError> {
        match self {
            Some(inner) => inner.write(record),
            None => Err(PortError(0)),
        }
    }
}

/// 戦略の戻り値。0は正常、1以上は戦略が定義する異常（EK-DD-001 2.8節）。
pub type StrategyStatus = u32;

/// 戦略（F-010）。
pub trait Strategy {
    /// 市場イベント、`OrderAck`、`Fill` のたびに呼ばれる。
    fn on_event(&mut self, context: &mut Context<'_>, event: EventRecord<'_>) -> StrategyStatus;
}
