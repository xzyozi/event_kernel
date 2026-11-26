---
title: "テスト計画・結果報告書"
document_type: "test_plan_report"
document_number: "EK-TEST-001"
document_role: "evidence"
ssot_scope: "test_evidence"
status: "draft"
version: "0.2"
created_at: "2026-11-26"
updated_at: "2026-11-27"
author: "xzyozi"
owner: "xzyozi"
purpose: "Deterministic Event Kernelの受け入れ条件（AC-01〜AC-08）、FFI検証スパイク（V-01〜V-06）、詳細設計・データ構造・外部契約の機能テスト要件に対するテスト計画・基準機・実行結果・証跡を管理するため"
related_documents:
  - "ドキュメントガバナンス（相対パス: ../README.md）"
  - "Deterministic Event Kernel システム要件定義書（相対パス: ../features/deterministic-event-kernel-requirements.md）"
  - "EK-BD-001 基本設計書（相対パス: ../design/EK-BD-001_基本設計書.md）"
  - "EK-DD-001 カーネルコア詳細設計書（相対パス: ../design/EK-DD-001_カーネルコア詳細設計書.md）"
  - "EK-DS-001 データ構造仕様書（相対パス: ../design/EK-DS-001_データ構造仕様書.md）"
  - "EK-IF-001 IF仕様書（相対パス: ../design/EK-IF-001_IF仕様書.md）"
  - "EK-ENV-001 環境・開発基盤仕様書（相対パス: ../setup/EK-ENV-001_環境・開発基盤仕様書.md）"
  - "EK-SETUP-001 環境構築手順書（相対パス: ../setup/EK-SETUP-001_環境構築手順書.md）"
  - "ADR-003 Python FFI境界方式の選定（相対パス: ../adr/003-python-ffi-boundary.md）"
  - "ADR-004 coreのメモリ確保方針（相対パス: ../adr/004-core-memory-allocation.md）"
---

# テスト計画・結果報告書
**Deterministic Event Kernel 検証計画・スパイク計画・実施結果**

---

## 1. 目的・対象範囲

- **目的**: Deterministic Event Kernel が要件定義書の受け入れ条件（`AC-01`〜`AC-08`）、非機能要件（`NFR-01`〜`NFR-05`）、ADR-003 の検証計画（`V-01`〜`V-06`）、および設計書の機能テスト要件（コア `TR-001`〜`TR-024`、外部契約 `TR-IF-001`〜`TR-IF-008`、DS 形式検査・原子書き込み）を満たすことを検証し、結果と証跡を記録する。
- **テスト対象**:
  - `core` クレート（`event-kernel-core` / `ek_core`: イベントループ、論理クロック、マージキュー、履歴リング、State Arena / `StrategyArena`、アクションキュー、Xoshiro256++ PRNG、XXH3-64 / SHA-256 ハッシュ・トレース、スナップショット生成・復元）。
  - 永続化・形式検査（スナップショット / トレースのバイナリ形式検査、およびアダプタ実装後の原子書き込み・`run_id` 別一時ファイル清掃）。
  - `core-capi`（C ABI 構造体・関数、`catch_unwind`、`Poisoned` 契約、`cbindgen` ヘッダ整合：今後実装）。
  - `bindings/python`（PyO3 API、`ViewGuard` 寿命、Python 例外変換、Arrow PyCapsule 出力と出力ハッシュ：今後実装）。
- **対象外**:
  - 個別の売買・シミュレーション戦略アルゴリズム自体の収益性や妥当性。
  - 本番環境へのリアルタイム発注・ネットワーク通信（要件定義書 1.2節アウトオブスコープ）。
  - Python 戦略コールバック実行時のゼロアロケーションおよび 16 並列決定論性（要件定義書 7章により `AC-04` / `AC-05` はネイティブ戦略のみが対象）。
- **関連する要求・設計**:
  - 要件定義書: [deterministic-event-kernel-requirements.md](../features/deterministic-event-kernel-requirements.md)（`FR-01`〜`FR-06`, `NFR-01`〜`NFR-05`, `AC-01`〜`AC-08`, 8章標準測定条件）
  - 基本設計書: [EK-BD-001](../design/EK-BD-001_基本設計書.md)
  - 詳細設計書: [EK-DD-001](../design/EK-DD-001_カーネルコア詳細設計書.md)（`TR-001`〜`TR-024`）
  - データ構造仕様書: [EK-DS-001](../design/EK-DS-001_データ構造仕様書.md)（3.1節原子書き込み, 4.3節形式検査）
  - IF/API仕様書: [EK-IF-001](../design/EK-IF-001_IF仕様書.md)（`TR-IF-001`〜`TR-IF-008`）
  - 環境仕様書: [EK-ENV-001](../setup/EK-ENV-001_環境・開発基盤仕様書.md)（4.4節品質ゲート `QG-001`〜`QG-014`）
  - 境界判断理由: [ADR-003](../adr/003-python-ffi-boundary.md)（`V-01`〜`V-06`）

## 2. テスト戦略・合格基準

該当するテスト方式だけを選択する。

- [x] TDD
- [x] 回帰テスト
- [x] 結合テスト
- [x] 性能・負荷テスト
- [x] 移行・復旧テスト（スナップショット復元・破損検出・原子置換復旧）
- [x] その他: メモリ安全性検証（Miri による未定義動作検査・カウンタ付きアロケータ検証）

- **選択理由**:
  - `core` は因果律・決定論性・ゼロアロケーション・状態遷移の不変条件が厳格なため、公開境界（Seam）に対して TDD で単体・結合テストを先行整備し、`cargo test` および CI で回帰保証する。
  - FFI 境界（`core-capi` / `bindings/python`）は C ABI と PyO3 の2面を持つため、同一シナリオの SHA-256 一致を結合契約テストで担保し、さらに Rust 側の `ViewGuard` 寿命を Miri で、Python 側を `pytest` で検証する。
  - スナップショット永続化は破損・中断・重複ファイル名に対する復元・拒否テストが不可欠なため、復旧テストを含める。
- **合格基準**:
  - 受け入れ条件 `AC-01`〜`AC-05`、`AC-07`、`AC-08` がすべて Pass すること。
  - `AC-06`（毎秒 1,000万イベント以上）は、基準機（`OPEN-06`）が確定するまで参考値として計測値を記録し、確定後に合否判定に組み込む（EK-ENV-001 2章）。
  - ADR-003 スパイク項目 `V-01`〜`V-06` の計測・検証を完了し、契約一致（`V-03`〜`V-06`）がすべて Pass すること。
  - 適用品質ゲートでエラー・警告ゼロを達成すること。
- **適用する品質ゲート**: [EK-ENV-001 環境・開発基盤仕様書](../setup/EK-ENV-001_環境・開発基盤仕様書.md) 4.4節の `QG-001`〜`QG-014` を正本として参照する（実行コマンド・合格基準は本書に再掲しない）。
- **使用する証跡**: CI 実行ログ（GitHub Actions Linux / Windows マトリクス、`ci-rust.yml` / `ci-python.yml`）、出力 SHA-256 ダイジェスト比較ログ、ベンチマーク結果記録、Miri 実行ログ。

### 2.1 テスト対象のSeam（公開境界）

| Seam / 公開インターフェース                     | 確認する振る舞い                                                                 | テスト対象外の内部実装                                 |
| :---------------------------------------------- | :------------------------------------------------------------------------------- | :----------------------------------------------------- |
| `core` 安全 Rust API（`create`/`step`/`run`/`submit_action`/`capture`/`restore`/`state_digest`/`destroy`） | 状態遷移、因果律・連番検証、ポート呼び出し順、ゼロアロケーション、ハッシュ一致 | 内部ヒープ配列の並び、リングバッファの回転インデックス |
| 4大ポート境界（`ClockPort`, `EventFeedPort`, `ExecutionPort`, `SnapshotPort`） | 2段階約定順序、`seek`/`restore_state` 全上書き、`validate_state` 無副作用       | テスト用モック/メモリ実装の内部コレクション構造        |
| `core-capi` C ABI（`ek_kernel_*`）              | NULL/サイズ検証、`catch_unwind` による `ERR_PANIC` 変換、`cbindgen` ヘッダ一致  | Rust 側内部ラッパの補助構造                            |
| `bindings/python`（`Kernel`, `Context`, ビュー群, Arrow PyCapsule） | Python 例外変換、`ViewGuard` 無効化（`RuntimeError`）、Arrow ゼロコピー出力    | PyO3 マクロが生成するグルーコード内部                  |

## 3. テスト環境・前提条件

- **環境**:
  - CI（Linux）: GitHub Actions `ubuntu-latest`（Rust `>= 1.85` + Python 3.12）
  - CI（Windows）: GitHub Actions `windows-latest`（OS 間 SHA-256 一致検証 `AC-07`）
  - 定期 / スパイク環境: Rust `nightly` + Miri（`QG-009`）
- **性能基準機記録欄（要件定義書 `OPEN-06`）**:
  - 確定状態: **未確定**（ベンチマーク実施環境を確定した時点で、CPU 型番・コア数・動作クロック・メモリ容量・OS/カーネル版数・Rust ツールチェーン版数を本欄へ記録する）。
- **テストデータ**:
  - 決定論的合成フィード（単調増加、同一時刻マルチストリーム、時刻逆行、連番違反、シーク不可フィード、`SnapshotTrigger` / `EndOfStream` フラグ付きデータ）。
  - 標準ベンチマークデータ（要件定義書 8章: 2ストリーム計 1,000万イベント、32バイトヘッダ + 64バイトペイロードのインメモリ SoA 構成）。機密情報・実顧客データは使用しない。
- **前提条件**:
  - Rust Cargo ワークスペースのうち `core/`・`Cargo.toml`・`Cargo.lock`・`rust-toolchain.toml`・`.github/workflows/ci-rust.yml`（`QG-001`〜`QG-006`）は `v0.0.1` で整備済み。`core-capi/`・`bindings/python/`・`adapters/` および `QG-007`〜`QG-009`・`QG-012`〜`QG-013` は未整備（`EK-ENV-001` 6章 `EX-001` / `EX-002`）。
- **実行方法**: `EK-ENV-001` 4.4節（`QG-001`〜`QG-014`）および [EK-SETUP-001](../setup/EK-SETUP-001_環境構築手順書.md) に従う。

## 4. テストケース

設計書・要件定義書・ADR-003 との追跡性を確保するため、期待結果の根拠に `AC-xxx` / `TR-xxx` / `TR-IF-xxx` / `V-xx` / `DS` 節番号を記載する。`v0.0.1` 時点で `core/` の機能テスト 53 件（`TC-001`〜`TC-016` のコア範囲および `TC-026`）は Pass 済みであり、`TC-017` は参考計測記録済み、`TC-018`〜`TC-025`（境界層）および `TC-016` のアダプタ永続化部分は未実施である。

| ケースID | 対象・条件                                  | 入力・操作                                                                             | 期待結果                                                                                                   | 期待結果の根拠            | 実績結果 | 判定   | 証跡 |
| :------- | :------------------------------------------ | :------------------------------------------------------------------------------------- | :--------------------------------------------------------------------------------------------------------- | :------------------------ | :------- | :----- | :--- |
| TC-001   | カーネル初期化（`create`）                  | 不正な設定値（`history_ring_len=0`、アラインメント違反、ストリーム数超過）と正常設定で初期化 | 不正時は `ERR_INVALID_ARGUMENT` / `ERR_CAPACITY_EXCEEDED` を返しハンドル未生成。正常時は `Ready` で `processed_event_count=0` | TR-001, TR-018            | 7件全合格（`create.rs` + `multi_feed.rs`） | Pass | `RUN-002` |
| TC-002   | ホットパス動的確保ゼロ（ネイティブ戦略）    | カウンタ付きグローバルアロケータ有効下で `create` 後に `run`（アクション・履歴・ハッシュ・保存復元含む）を実行 | `create` 完了後から `run` 終了までのヒープ確保・解放回数が `0` 回                                          | AC-05, NFR-02, TR-002     | 4件全合格（`alloc_free.rs`） | Pass | `RUN-002` |
| TC-003   | 同時刻イベント・仮想ストリーム順序          | 複数外部フィード＋システムストリーム（`0xFFFF_FF00`）から同一時刻イベントを投入し `step` | 常に `(timestamp_ns, stream_id, sequence_id)` 昇順で配送され、システムストリームは外部フィードの後に配送   | AC-02, FR-01-3, TR-003    | `ordering.rs`・`multi_feed.rs` で全合格 | Pass | `RUN-002` |
| TC-004   | 因果律違反（時刻逆行）                      | 現在時刻 $t_{curr}$ より小さい `timestamp_ns` のイベントを供給して `step`              | イベントを処理せず `ERR_CAUSALITY_VIOLATION` を返し、`Poisoned` に遷移する                                 | AC-01, FR-01-2, TR-004    | `causality.rs`・`multi_feed.rs` で全合格 | Pass | `RUN-002` |
| TC-005   | 連番違反（同一ストリーム）                  | 同一ストリーム内で `(timestamp_ns, sequence_id)` が増加しないイベントを供給して `step` | イベントを処理せず `ERR_SEQUENCE_VIOLATION` を返し、`Poisoned` に遷移する                                  | DS 2.2節, TR-005          | `causality.rs`・`multi_feed.rs` で全合格 | Pass | `RUN-002` |
| TC-006   | 同一市場イベントでの2段階処理順序           | 待機中アクションがある状態で市場イベントを供給し、`ExecutionPort` と戦略の呼び出し順を記録 | `ExecutionPort.on_market_event` → `poll_result` が戦略通知より先に呼ばれ、市場イベントのみ履歴に入る     | BD 2.3節, TR-006, TR-020  | `ordering.rs` で全合格 | Pass | `RUN-002` |
| TC-007   | アクション遅延・未来約定禁止                | `latency_ns > 0`、`latency_ns <= 0`、`i64::MAX` 超過で `submit_action` を実行          | `<= 0` やオーバーフローは `ERR_INVALID_ARGUMENT`。正値時は `now_ns + latency_ns` より前に約定しない        | AC-02, FR-04-1/2, TR-007, TR-024 | `ordering.rs` で合格（独立 `ClockPort::advance_to` 単体テストは今後補強） | Pass（条件付き） | `RUN-002` |
| TC-008   | アクションキュー満杯                        | `action_queue_capacity` 上限到達後に `submit_action` を呼び出す                        | `ERR_QUEUE_FULL` を返し、状態ハッシュが変化せず、`Poisoned` に遷移しない                                   | AC-08, TR-008             | `ordering.rs` で `ERR_QUEUE_FULL` と `Ready` 維持合格（状態ハッシュ比較は今後補強） | Pass（条件付き） | `RUN-002` |
| TC-009   | 戦略・結果エラー時の保全                    | 戦略コールバックエラー、過去時刻結果、不正 `event_type` 結果を返却                     | `ERR_STRATEGY_FAILURE`（`detail` 設定）/ `ERR_CAUSALITY_VIOLATION` / `ERR_PORT_FAILURE` で `Poisoned`      | BD 3.2節, TR-009, TR-019  | `failures.rs` で全合格 | Pass | `RUN-002` |
| TC-010   | スナップショット保存・分岐・復元一致        | 任意時点で `capture` → 別パラメータで分岐実行 → `restore` → 同一条件で再実行           | 元の実行の最終状態・`HISTORY_RING`・出力・PRNG 系列・トレースとビット単位（SHA-256）で完全一致           | AC-03, FR-03-2, TR-010, TR-017, TR-021 | `snapshot_restore.rs`・`prng.rs`・`rng_wiring.rs` で全合格 | Pass | `RUN-002` |
| TC-011   | 非 seekable フィード / 最大長超過 / IO失敗  | `seekable=false` フィード、`EXEC_STATE`/トークン長超過、`SnapshotPort::store` 失敗で `capture` | `ERR_SNAPSHOT_UNSUPPORTED` / `ERR_SNAPSHOT_IO` を返し状態不変。`state_digest` は非 seekable でも成功      | FR-03-2, TR-011, TR-023   | `snapshot_restore.rs`・`determinism.rs` で全合格 | Pass | `RUN-002` |
| TC-012   | `restore` 検証失敗・適用失敗・`Poisoned` 復旧 | 不正形式・設定不一致で `restore`、適用途中失敗で `restore`、`Poisoned` 状態で正常 `restore` | 検証失敗は状態不変。適用失敗は `Poisoned`。`Poisoned` からの正常 `restore` は `Ready` 復帰・結果一致     | ADR-003 5.1-9, TR-012, TR-013 | `snapshot_restore.rs` で全合格 | Pass | `RUN-002` |
| TC-013   | `Running` 中の再入拒否                      | 戦略コールバック内部から `step` / `run` / `capture` / `restore` / `destroy` 等を呼び出す | すべて `ERR_REENTRANT` を返し、カーネル状態を変更しない                                                    | BD 2.4節, TR-014          | `failures.rs`・`snapshot_restore.rs`・`strategy_arena.rs` で全合格 | Pass | `RUN-002` |
| TC-014   | `run` 停止条件別の終了コード                | 終端到達、`max_events` 到達、`SnapshotTrigger` フラグ付きイベントで `run` を実行       | それぞれ `STEP_FINISHED`、`RUN_LIMIT_REACHED`、`RUN_SNAPSHOT_REQUESTED` を返す                             | TR-015                    | `failures.rs` で全合格 | Pass | `RUN-002` |
| TC-015   | 単一実行・16並列実行・OS間決定論性          | 同一入力・シードで単一実行、16スレッド並列実行、Linux / Windows 各 CI で実行           | 正準バイト列 SHA-256、トレースログ SHA-256、Arrow 出力ハッシュ（境界実装後）が 100% 一致                   | AC-04, AC-07, NFR-01, TR-016, TR-IF-008 | `core/`（`determinism.rs`・`hash.rs`・`ubuntu`/`windows` CI）は Pass。Arrow 出力は未実施 | Pass（`core/` 分） | `RUN-002` |
| TC-016   | スナップショット・トレース形式検査・原子書き込み | 破損バイト列、非昇順セクション、`reserved != 0`、`run_id` 同名衝突、異常終了後の一時ファイル清掃 | 形式違反は読み込み拒否。同名衝突・清掃はアダプタ層実装後に検証                                             | DS 3.1節, DS 4.3節        | DS 4.3節形式検査は `snapshot_restore.rs`・`hash.rs` で合格。DS 3.1節原子書き込みは未実施 | Pass（形式検査のみ） | `RUN-002` |
| TC-017   | NFR-03 標準ベンチマーク（参考計測→判定）    | 要件定義書 8章の標準条件（2ストリーム 1,000万イベント, `--release` LTO, 単一物理コア）  | 毎秒 1,000万イベント以上（`OPEN-06` 基準機確定前は参考値記録、確定後に合否判定）                           | AC-06, NFR-03, QG-008     | 参考値: Apple M3 / 2-stream+trace+xxh3(10k) 中央値 **56,002,449 events/s**（`コア縦切り実装計画.md` 7.7節） | 判定保留（参考 Pass） | `コア縦切り実装計画.md` |
| TC-018   | C ABI レイアウト・`cbindgen` ヘッダ一致     | `EkEventHeader` のサイズ/オフセット静的検査および `cbindgen` 出力とコミットヘッダ比較  | サイズ 32バイト、8バイト境界、全フィールドオフセットが DS と一致し、生成ヘッダ差分ゼロ                     | ADR-003 5.1-1, QG-012, TR-IF-001 | 未実施   | 未実施 | なし |
| TC-019   | C ABI 不正引数防御                          | NULL ポインタ、`struct_size` 不一致、`trace_enabled=2` を渡して C ABI 関数を呼び出す  | `ERR_INVALID_ARGUMENT` を返し、未定義動作・クラッシュを起こさない                                          | TR-IF-002                 | 未実施   | 未実施 | なし |
| TC-020   | `V-01`: Python コールバック単位比較スパイク | イベント単位コールバックとバッチ単位コールバックで同一入力を実行しスループットを比較   | 処理時間・境界通過コストを実測記録し、バッチ単位の既定値根拠とする                                         | ADR-003 V-01              | 未実施   | 未実施 | なし |
| TC-021   | `V-02`: Arrow PyCapsule ゼロコピー検証      | `export_fills` 等から Polars / PyArrow へ受け渡し、データポインタアドレスと解放を確認  | バッファアドレスが一致（追加コピーなし）、Python オブジェクト破棄時に `release` コールバックが実行される   | ADR-003 V-02, TR-IF-007   | 未実施   | 未実施 | なし |
| TC-022   | `V-03`: C ABI / Python 出力 SHA-256 一致    | 同一シナリオ・同一シードを `core-capi` と `bindings/python` の両方から実行して比較     | 最終状態 `state_digest`、トレース、Arrow 出力の SHA-256 が完全一致                                         | ADR-003 V-03, QG-007, TR-IF-003 | 未実施   | 未実施 | なし |
| TC-023   | `V-04` / `V-06`: パニック・例外と `Poisoned` | コールバック内でパニック / Python 例外を発生させ、その後 `step` / `capture` / `submit_action` を呼ぶ | 未捕捉クラッシュせず `ERR_PANIC` / 例外へ変換され、以後の操作が両境界で `ERR_POISONED` 相当になる          | ADR-003 V-04, V-06, QG-007, TR-IF-004, TR-IF-005 | 未実施   | 未実施 | なし |
| TC-024   | `V-05`: `ViewGuard` メモリ安全性（Rust/Miri） | Rust 側で入力ビュー・`StrategyArena` ビューをコールバック後に参照するテストを Miri で実行 | Miri 上で未定義動作（UB）が検出されず、無効化フラグにより拒否される                                        | ADR-003 V-05, QG-009      | 未実施   | 未実施 | なし |
| TC-025   | `V-05`: `ViewGuard` 寿命検査（Python）      | 正常返却後および Python 例外送出後に保持したビューと導出 `memoryview` へアクセス       | すべての経路で `RuntimeError` が送出される                                                                 | ADR-003 V-05, QG-013, TR-IF-006 | 未実施   | 未実施 | なし |
| TC-026   | `StrategyArena` 初期化と読み出し（F-011/F-012） | 開始前の初期化、処理後・`Finished`・境界外・`Poisoned` での拒否、ゼロアロケーション読み出し | `Ready`（`processed_event_count=0`）時のみ書込成功、`Finished` 書込拒否、読み出しは状態不変・確保 0 回   | TR-022                    | 7件全合格（`strategy_arena.rs`） | Pass | `RUN-002` |

## 5. 実施結果

| 実行ID  | 実行日     | 実行者・実行環境 | 実行方法                                       | 結果概要                                                   | 証跡                               |
| :------ | :--------- | :--------------- | :--------------------------------------------- | :--------------------------------------------------------- | :--------------------------------- |
| RUN-001 | 2026-11-26 | xzyozi（設計段階） | `python3 .github/scripts/validate_docs.py` 等  | ドキュメント構造・整合性検証のみ実施。コード・Rust CI は未実装 | 本ファイル作成時点の計画登録のみ   |
| RUN-002 | 2026-11-27 | xzyozi（macOS / GitHub Actions Ubuntu & Windows） | `cargo fmt --all --check`、`cargo clippy --workspace --all-targets --locked -- -D warnings`、`check_core_attrs.py`、`cargo test --workspace --locked`、PR #12 全14チェック | `core/` 統合テスト **53 件全合格**（0件失敗）、`QG-001`〜`QG-006` / `QG-010` / `QG-011` 全合格（`v0.0.1`） | GitHub Release tag `v0.0.1` / PR #12 / ローカル実行ログ |

- **総合判定**: **`core/` は合格 / システム全体としては条件付き判定保留**
- **判定理由**: `v0.0.1` で Rust コア（`core/`）の単体・結合テスト 53 件および Linux / Windows CI（`QG-001`〜`QG-006`）は全合格した。一方、FFI 境界層（`core-capi/`・`bindings/python/`）と `V-01`〜`V-06` スパイク、アダプタ層の原子書き込み検証、および `OPEN-06` 正式基準機での `AC-06` 判定が未実施であるため、全体としては境界層・基準機確定待ちとする。

## 6. 未実施・対象外・テスト差異

| ID     | 対象項目                           | 状態   | 理由・標準との差異                                                       | 影響・リスク                                               | 代替確認・対応                                                 | 承認者・期限           |
| :----- | :--------------------------------- | :----- | :----------------------------------------------------------------------- | :--------------------------------------------------------- | :------------------------------------------------------------- | :--------------------- |
| EX-001 | `core` / Rust 単体・結合テスト全般 | 解除済み（`core/`） | `v0.0.1` で `core/` と `ci-rust.yml`（`QG-001`〜`QG-006`）を整備・実行完了 | `core/` 単体の決定論性・ゼロアロケーション・因果律・復元は検証済み | `core-capi/`・`bindings/python/`・`adapters/` のテストは各層 PR で追加 | xzyozi / 完了（`core/`） |
| EX-002 | `V-01`〜`V-06` FFI スパイク        | 未実施 | ADR-003 は `proposed` であり、境界プロトタイプが未実装                   | `bindings/python` と `core-capi` の性能・PyCapsule 実動作が未確認 | スパイク PR にて実測し、結果を ADR-003 と本書 RUN 履歴へ追記   | xzyozi / ADR-003 承認前 |
| EX-003 | `AC-06` 性能ベンチマーク           | 参考計測済み（正式判定保留） | 正式基準機が未確定（要件定義書 `OPEN-06`） | 参考環境（Core i5-1235U Linux VM / Apple M3）では 1,000万 events/s を超過したが、正式環境での基準値が未確定 | `OPEN-06` 確定後に `QG-008` 標準ベンチマークで正式合否判定を実施 | xzyozi / 基準機確定時  |

## 7. 関連文書

- **要求・機能仕様**: [要件定義書](../features/deterministic-event-kernel-requirements.md)
- **基本・詳細・データ構造・IF設計**: [EK-BD-001 基本設計書](../design/EK-BD-001_基本設計書.md)、[EK-DD-001 カーネルコア詳細設計書](../design/EK-DD-001_カーネルコア詳細設計書.md)、[EK-DS-001 データ構造仕様書](../design/EK-DS-001_データ構造仕様書.md)、[EK-IF-001 IF仕様書](../design/EK-IF-001_IF仕様書.md)
- **環境仕様・構築手順**: [EK-ENV-001 環境・開発基盤仕様書](../setup/EK-ENV-001_環境・開発基盤仕様書.md)、[EK-SETUP-001 環境構築手順書](../setup/EK-SETUP-001_環境構築手順書.md)
- **実装記録・ADR**: [コア縦切り実装計画](../analysis/コア縦切り実装計画.md)、[ADR-003 Python FFI境界方式の選定](../adr/003-python-ffi-boundary.md)、[ADR-004 coreのメモリ確保方針](../adr/004-core-memory-allocation.md)

## 8. 作成完了チェック

- [x] `document_role`、`ssot_scope`、`status`、`owner` を Frontmatter へ設定した。
- [x] 目的、対象範囲、対象外、関連する要求・設計を記入した。
- [x] 採用したテスト戦略と、プロジェクト固有の合格基準を記入した。
- [x] 対象 Seam（公開境界）と検証範囲を記載した。
- [x] テストケースに期待結果、根拠（`AC`/`TR`/`V`/`DS` ID）、実績、判定、証跡枠を記録した。
- [x] 品質ゲートは `EK-ENV-001` を参照し、実行コマンドや合格基準を再掲していない。
- [x] 未実施・対象外・差異の理由と影響を `EX-001`〜`EX-003` として記録した。
- [x] テスト文書が要求・設計の正本を上書きしていないことを確認した。
- [x] 秘密情報や利用者固有の認証情報を記載していない。
- [x] `EK-DD-001` および `EK-IF-001` への相対リンクを追加した。

## 9. 改訂履歴 (Change Log)

| 版数    | 改訂日     | 変更者 | 変更内容・変更理由 (Why)                                                                                               |
| :------ | :--------- | :----- | :--------------------------------------------------------------------------------------------------------------------- |
| Rev.0.1 | 2026-11-26 | xzyozi | 初版ドラフト。要件定義書 `AC-01`〜`AC-08`、ADR-003 `V-01`〜`V-06`、DD/DS/IF 機能テスト要件に対応するテスト計画を策定。 |
| Rev.0.2 | 2026-11-27 | xzyozi | `EK-DD-001`・`EK-IF-001`・`EK-SETUP-001` リンク追加、および `v0.0.1` の `core/` 53件合格・CI・H1 参考値（`RUN-002`, `TC-001`〜`TC-017`, `TC-026`）を反映。 |
