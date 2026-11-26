//! トレースログのヘッダとレコード（EK-DS-001 2.2節）。

/// トレースヘッダの先頭マジックバイト（`EKTR`）。
pub const TRACE_MAGIC: [u8; 4] = *b"EKTR";
/// トレース形式の主版数。
pub const TRACE_FORMAT_MAJOR: u16 = 1;
/// トレース形式の副版数。
pub const TRACE_FORMAT_MINOR: u16 = 0;
/// `hash_algorithm`: XXH3-64（EK-DS-001 2.2節、OPEN-05）。
pub const TRACE_HASH_ALGORITHM_XXH3_64: u16 = 1;
/// `trace_flags` の bit0: `HashPresent`。
pub const TRACE_FLAG_HASH_PRESENT: u16 = 0x0001;

/// トレースログのヘッダ（32バイト）。配置は EK-DS-001 2.2節を正本とする。
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TraceHeader {
    /// マジック（`EKTR`）。
    pub magic: [u8; 4],
    /// 主版数。
    pub format_major: u16,
    /// 副版数。
    pub format_minor: u16,
    /// `TRACE_RECORD` のバイト数（40）。
    pub record_size: u32,
    /// 状態ハッシュのアルゴリズム（1: XXH3-64）。
    pub hash_algorithm: u16,
    /// 予約領域（0）。
    pub reserved0: u16,
    /// 状態ハッシュを計算するイベント数の間隔。0は計算しない。
    pub hash_interval: u64,
    /// 予約領域（0）。
    pub reserved1: [u8; 8],
}

const _: () = assert!(core::mem::size_of::<TraceHeader>() == TraceHeader::SIZE);
const _: () = assert!(core::mem::align_of::<TraceHeader>() == 8);

impl TraceHeader {
    /// ヘッダのバイト数。
    pub const SIZE: usize = 32;

    /// 既定値で新しい `TraceHeader` を作る。
    pub const fn new(hash_interval: u64) -> Self {
        Self {
            magic: TRACE_MAGIC,
            format_major: TRACE_FORMAT_MAJOR,
            format_minor: TRACE_FORMAT_MINOR,
            record_size: TraceRecord::SIZE as u32,
            hash_algorithm: TRACE_HASH_ALGORITHM_XXH3_64,
            reserved0: 0,
            hash_interval,
            reserved1: [0; 8],
        }
    }

    /// EK-DS-001 2.2節のリトルエンディアン 32 バイト表現へ変換する。
    pub fn to_le_bytes(&self) -> [u8; Self::SIZE] {
        let mut out = [0u8; Self::SIZE];
        if let Some(slot) = out.get_mut(0..4) {
            slot.copy_from_slice(&self.magic);
        }
        if let Some(slot) = out.get_mut(4..6) {
            slot.copy_from_slice(&self.format_major.to_le_bytes());
        }
        if let Some(slot) = out.get_mut(6..8) {
            slot.copy_from_slice(&self.format_minor.to_le_bytes());
        }
        if let Some(slot) = out.get_mut(8..12) {
            slot.copy_from_slice(&self.record_size.to_le_bytes());
        }
        if let Some(slot) = out.get_mut(12..14) {
            slot.copy_from_slice(&self.hash_algorithm.to_le_bytes());
        }
        if let Some(slot) = out.get_mut(14..16) {
            slot.copy_from_slice(&self.reserved0.to_le_bytes());
        }
        if let Some(slot) = out.get_mut(16..24) {
            slot.copy_from_slice(&self.hash_interval.to_le_bytes());
        }
        if let Some(slot) = out.get_mut(24..32) {
            slot.copy_from_slice(&self.reserved1);
        }
        out
    }
}

/// トレースレコード（40バイト）。配置は EK-DS-001 2.2節を正本とする。
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TraceRecord {
    /// 処理した入力イベントの時刻。
    pub timestamp_ns: i64,
    /// 処理した入力イベントの `sequence_id`。
    pub sequence_id: u64,
    /// 処理した入力イベントの `stream_id`。
    pub stream_id: u32,
    /// 処理した入力イベントの `event_type`。
    pub event_type: u16,
    /// 処理した入力イベントの `flags`。
    pub event_flags: u16,
    /// 状態ハッシュ（`HashPresent` が0のときは0）。
    pub state_hash: u64,
    /// `trace_flags`（bit0 `HashPresent`）。
    pub trace_flags: u16,
    /// 予約領域（0）。
    pub reserved: [u8; 6],
}

const _: () = assert!(core::mem::size_of::<TraceRecord>() == TraceRecord::SIZE);
const _: () = assert!(core::mem::align_of::<TraceRecord>() == 8);

impl TraceRecord {
    /// レコードのバイト数。
    pub const SIZE: usize = 40;

    /// EK-DS-001 2.2節のリトルエンディアン 40 バイト表現へ変換する。
    pub fn to_le_bytes(&self) -> [u8; Self::SIZE] {
        let mut out = [0u8; Self::SIZE];
        if let Some(slot) = out.get_mut(0..8) {
            slot.copy_from_slice(&self.timestamp_ns.to_le_bytes());
        }
        if let Some(slot) = out.get_mut(8..16) {
            slot.copy_from_slice(&self.sequence_id.to_le_bytes());
        }
        if let Some(slot) = out.get_mut(16..20) {
            slot.copy_from_slice(&self.stream_id.to_le_bytes());
        }
        if let Some(slot) = out.get_mut(20..22) {
            slot.copy_from_slice(&self.event_type.to_le_bytes());
        }
        if let Some(slot) = out.get_mut(22..24) {
            slot.copy_from_slice(&self.event_flags.to_le_bytes());
        }
        if let Some(slot) = out.get_mut(24..32) {
            slot.copy_from_slice(&self.state_hash.to_le_bytes());
        }
        if let Some(slot) = out.get_mut(32..34) {
            slot.copy_from_slice(&self.trace_flags.to_le_bytes());
        }
        if let Some(slot) = out.get_mut(34..40) {
            slot.copy_from_slice(&self.reserved);
        }
        out
    }
}
