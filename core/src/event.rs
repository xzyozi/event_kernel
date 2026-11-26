//! イベントのヘッダとレコード（EK-DS-001 2.2節、2.4節）。

/// `EventHeader`（32バイト）。配置は EK-DS-001 2.2節を正本とする。
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EventHeader {
    /// 論理発生時刻（ナノ秒）。
    pub timestamp_ns: i64,
    /// 同一ストリーム内の連番。
    pub sequence_id: u64,
    /// データ源の識別子。
    pub stream_id: u32,
    /// イベント種別（`event_type`）。
    pub event_type: u16,
    /// フラグ（`flags`）。
    pub flags: u16,
    /// 外部フィードのペイロード配列内の添字。
    pub payload_idx: u64,
}

const _: () = assert!(core::mem::size_of::<EventHeader>() == EventHeader::SIZE);
const _: () = assert!(core::mem::align_of::<EventHeader>() == 8);

impl EventHeader {
    /// ヘッダのバイト数。
    pub const SIZE: usize = 32;

    /// 配送順序の比較キー `(timestamp_ns, stream_id, sequence_id)`（EK-DD-001 2.5節）。
    #[inline]
    pub fn order_key(&self) -> (i64, u32, u64) {
        (self.timestamp_ns, self.stream_id, self.sequence_id)
    }
}

/// イベント種別（EK-DS-001 2.4節）。
pub mod event_type {
    /// 価格ティック。
    pub const TICK: u16 = 1;
    /// バー（OHLC）。
    pub const BAR: u16 = 2;
    /// 板情報。
    pub const BOOK_UPDATE: u16 = 3;
    /// 指標データ。
    pub const INDICATOR: u16 = 4;
    /// タイマーイベント。
    pub const TIMER: u16 = 5;
    /// アクションの到達イベント。
    pub const ACTION_ARRIVAL: u16 = 16;
    /// 注文の受付応答。
    pub const ORDER_ACK: u16 = 17;
    /// 約定結果。
    pub const FILL: u16 = 18;

    /// 市場イベント（1〜15）かどうか。
    #[inline]
    pub const fn is_market(event_type: u16) -> bool {
        matches!(event_type, 1..=15)
    }
}

/// `flags` のビット（EK-DS-001 2.2節）。
pub mod flags {
    /// bit0 `SnapshotTrigger`。
    pub const SNAPSHOT_TRIGGER: u16 = 0x0001;
    /// bit1 `EndOfStream`。
    pub const END_OF_STREAM: u16 = 0x0002;
}

/// アクションキューの仮想ストリームの `stream_id`（EK-DD-001 2.5節）。
pub const ACTION_STREAM_ID: u32 = 0xFFFF_FF00;

/// `ActionArrival` のペイロードで、`action_payload` が始まるオフセット（EK-DS-001 2.4節）。
pub const ACTION_PAYLOAD_OFFSET: usize = 8;

/// `EVENT_RECORD` の読み取り専用ビュー。コールバック中だけ有効。
#[derive(Clone, Copy, Debug)]
pub struct EventRecord<'a> {
    /// ヘッダ。
    pub header: EventHeader,
    /// ペイロード（`payload_record_size` バイト）。
    pub payload: &'a [u8],
}