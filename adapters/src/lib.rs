//! Deterministic Event Kernel 向け外部アダプタ群（EK-BD-001 2.1節、EK-DS-001 3.1節）。
//!
//! - `feed`: インメモリ SoA / CSV / バイナリ / Apache Parquet フィード（seekable 契約準拠）
//! - `execution`: 決定論シミュレーション約定アダプタ（状態シリアライズ/復元対応）
//! - `persistence`: スナップショット原子書き込み・復旧検証（DS 3.1 / 4.3節）およびトレースログ読み書き

#![forbid(unsafe_code)]

pub mod execution;
pub mod feed;
pub mod persistence;

pub use execution::SimExecutionAdapter;
pub use feed::{
    BinarySoaFeed, CsvFeed, EventInputRecord, ParquetFeed, VecFeedAdapter, write_parquet_events,
};
pub use persistence::{
    DiskSnapshotStore, DiskTraceSink, MemorySnapshotStore, SnapshotKey, TraceLog, TraceLogReader,
    validate_run_id,
};
