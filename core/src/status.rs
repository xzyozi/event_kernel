//! 終了コードとエラーコード（EK-DD-001 2.4節、4.1節）。0以上は正常、負は失敗である。

/// 正常終了。
pub const OK: i32 = 0;
/// 入力が尽きた（正常）。
pub const STEP_FINISHED: i32 = 1;
/// `max_events` 個のイベントを処理した（正常）。
pub const RUN_LIMIT_REACHED: i32 = 2;
/// `SnapshotTrigger` 付きのイベントを処理した（正常）。
pub const RUN_SNAPSHOT_REQUESTED: i32 = 3;

/// 引数が制約を満たさない。
pub const ERR_INVALID_ARGUMENT: i32 = -1;
/// 導出される確保量が上限を超える。
pub const ERR_CAPACITY_EXCEEDED: i32 = -2;
/// アクションキューが満杯。
pub const ERR_QUEUE_FULL: i32 = -3;
/// 時刻が逆行した。
pub const ERR_CAUSALITY_VIOLATION: i32 = -4;
/// 同一時刻で `sequence_id` が増加しない。
pub const ERR_SEQUENCE_VIOLATION: i32 = -5;
/// `Poisoned` のハンドルへの操作。
pub const ERR_POISONED: i32 = -6;
/// `Running` 中の禁止操作。
pub const ERR_REENTRANT: i32 = -7;
/// スナップショットを作れない。
pub const ERR_SNAPSHOT_UNSUPPORTED: i32 = -8;
/// スナップショットの形式が不正。
pub const ERR_SNAPSHOT_INVALID: i32 = -9;
/// スナップショットの設定が一致しない。
pub const ERR_SNAPSHOT_MISMATCH: i32 = -10;
/// ポートの操作が失敗した。
pub const ERR_PORT_FAILURE: i32 = -11;
/// `ExecutionPort` の結果がアクションキューに入らない。
pub const ERR_RESULT_OVERFLOW: i32 = -12;
/// 戦略が1以上を返した。
pub const ERR_STRATEGY_FAILURE: i32 = -13;
/// スナップショットの保存に失敗した。
pub const ERR_SNAPSHOT_IO: i32 = -14;
/// パニックを捕捉した（縦切りでは、内部の不変条件の破れにも使う）。
pub const ERR_PANIC: i32 = -99;