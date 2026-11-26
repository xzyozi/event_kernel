//! ポートと戦略の呼び出し契約（EK-DD-001 2.7節、2.8節）。縦切りの範囲の操作だけを持つ。

use crate::event::{EventHeader, EventRecord};
use crate::history::HistoryView;
use crate::kernel::Context;

/// ポートの失敗。値は、ポートが定義する詳細コードで、`StepResult.detail` に入る。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PortError(pub i32);

/// 外部フィード（F-008）。`position` と `seek` は、縦切りの範囲外。
pub trait EventFeedPort {
    /// 次のイベントの時刻を返す。`None` は終端。
    fn peek_next_timestamp(&mut self) -> Result<Option<i64>, PortError>;

    /// 次のイベントを取り出し、ヘッダとペイロードを `core` のバッファへコピーする。
    fn pop_next(&mut self, header: &mut EventHeader, payload: &mut [u8]) -> Result<(), PortError>;
}

/// 約定判定（F-009）。`capture_state` などは、縦切りの範囲外。
pub trait ExecutionPort {
    /// `ActionArrival` を配送するときに呼ばれる。
    fn on_arrival(&mut self, event: EventRecord<'_>) -> Result<(), PortError>;

    /// 市場イベントを配送するときに、履歴への追加と戦略への通知より前に呼ばれる。
    /// `history` は、今のイベントを含まない。
    fn on_market_event(
        &mut self,
        event: EventRecord<'_>,
        history: HistoryView<'_>,
    ) -> Result<(), PortError>;

    /// 積まれた結果を、生成した順に1件ずつ返す。結果がなければ `Ok(false)`。
    fn poll_result(&mut self, header: &mut EventHeader, payload: &mut [u8])
    -> Result<bool, PortError>;
}

/// 戦略の戻り値。0は正常、1以上は戦略が定義する異常（EK-DD-001 2.8節）。
pub type StrategyStatus = u32;

/// 戦略（F-010）。
pub trait Strategy {
    /// 市場イベント、`OrderAck`、`Fill` のたびに呼ばれる。
    fn on_event(&mut self, context: &mut Context<'_>, event: EventRecord<'_>) -> StrategyStatus;
}