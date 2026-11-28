# Deterministic Event Kernel (`event_kernel`)

決定論的リプレイと先読みバイアス（Look-ahead Bias）の構造的排除を両立させる、イベント駆動カーネル基盤です。

- **リリース**: `v0.0.1`（`main`：`core/` 実装・検証済み、`develop`：`core-capi/`・`adapters/`・`bindings/python/` 実装および全品質ゲート整備完了）
- **言語構成**: Rust コア（`core/`、`#![no_std]` + `#![forbid(unsafe_code)]` + 初期化時のみ `alloc`）、C ABI（`core-capi/`）、外部ポートアダプタ（`adapters/`）、Python ホスト連携（`bindings/python/`、PyO3 + Arrow PyCapsule）

## リポジトリ構成

- `core/` (`event-kernel-core` / `ek_core`): カーネル本体（イベントループ、K-way マージ、履歴リングバッファ、アクションキュー、Xoshiro256++ PRNG、XXH3-64 / SHA-256 トレース、スナップショット保存・復元、`ViewLease`/`ViewGuard`）とテスト・ベンチマーク
- `core-capi/` (`event-kernel-capi` / `ek_capi`): `cbindgen` 生成 C ヘッダ（`core-capi/include/event_kernel.h`）付き C ABI 境界層
- `adapters/` (`event-kernel-adapters` / `ek_adapters`): Parquet / CSV / バイナリ / メモリフィード、原子ディスクスナップショット保存（`.eksnap`）、トレース永続化（`.ektrace`）、シミュレーション約定アダプタ
- `bindings/python/` (`event_kernel._core`): PyO3 拡張、`StrategyContext` / ビュー寿命管理、Arrow PyCapsule（`__arrow_c_stream__`）、契約テスト
- `docs/`: 要件定義、ADR、基本/詳細/データ構造/IF 設計書、環境仕様・構築手順書、テスト計画・結果報告書（SSOT 運用は [docs/README.md](docs/README.md) を参照）
- `.github/`: Rust / FFI（Ubuntu/Windows）/ Benchmark / Miri / Security Audit / Python / ドキュメント / Mermaid CI ワークフローと検証スクリプト

## 主要ドキュメント（正本インデックス）

| 領域 | 文書 | 位置づけ |
| :--- | :--- | :--- |
| 文書統治・用語 | [docs/README.md](docs/README.md) / [CONTEXT.md](CONTEXT.md) | 文書分類・SSOT・用語定義 |
| 要件定義 | [deterministic-event-kernel-requirements.md](docs/features/deterministic-event-kernel-requirements.md) | 機能要件・非機能要件・受け入れ条件（`AC-01`〜`AC-08`） |
| 判断記録 (ADR) | [ADR-001](docs/adr/001-document-governance-and-ssot.md) / [ADR-002](docs/adr/002-core-language-rust.md) / [ADR-003](docs/adr/003-python-ffi-boundary.md) / [ADR-004](docs/adr/004-core-memory-allocation.md) | SSOT・Rust 採用・Python FFI 境界案・メモリ確保方針 |
| 基本設計 (BD) | [EK-BD-001_基本設計書.md](docs/design/EK-BD-001_基本設計書.md) | ヘキサゴナル構成・責務境界・保全方針 |
| 詳細設計 (DD) | [EK-DD-001_カーネルコア詳細設計書.md](docs/design/EK-DD-001_カーネルコア詳細設計書.md) | 関数契約・ステップ処理手順・状態遷移・機能テスト要件 |
| データ構造 (DS) | [EK-DS-001_データ構造仕様書.md](docs/design/EK-DS-001_データ構造仕様書.md) | `EventHeader`・スナップショット/トレース形式・原子書き込み |
| 外部 IF (IF) | [EK-IF-001_IF仕様書.md](docs/design/EK-IF-001_IF仕様書.md) | `core-capi` C ABI・`bindings/python` API・Arrow 出力契約 |
| 環境仕様・手順 | [EK-ENV-001](docs/setup/EK-ENV-001_環境・開発基盤仕様書.md) / [EK-SETUP-001](docs/setup/EK-SETUP-001_環境構築手順書.md) | ツールチェーン・品質ゲート（`QG-001`〜`QG-014`）・構築手順 |
| テスト計画・結果 | [EK-TEST-001_テスト計画・結果報告書.md](docs/test/EK-TEST-001_テスト計画・結果報告書.md) | 検証計画・`TC-001`〜`TC-026` 検証実績・ベンチ/Miri/FFI証跡 |
| 実装計画・実測記録 | [コア縦切り実装計画.md](docs/analysis/コア縦切り実装計画.md) | `core/` 段階的実装の判断経緯とベンチマーク記録 |

## Rust から利用する（サブモジュール等）

利用側の `Cargo.toml` に `core/` への path 依存を追加します（crate は公開していません）。コード中のライブラリ名は `ek_core` です。Rust 1.97 以上が必要です。

```toml
[dependencies]
event-kernel-core = { path = "external/event_kernel/core" }
```

`core/` は、入出力を trait（ポート）で受け取ります。既製のフィード実装・約定実装は提供していないため、利用側で次を実装して `Kernel::create(config, feed, exec, strategy)` に渡します（詳細は [EK-DD-001](docs/design/EK-DD-001_カーネルコア詳細設計書.md) 2.7 節、2.8 節と `core/src/ports.rs`）。

| trait | 利用側の実装 | 必須メソッド | 既定実装があるメソッド |
| :--- | :--- | :--- | :--- |
| `EventFeedPort` | 必須 | `peek_next_timestamp`, `pop_next` | `position`, `validate_position`, `seek`（既定は位置を戻せないフィード） |
| `ExecutionPort` | 必須 | `on_arrival`, `on_market_event`, `poll_result` | `capture_state`, `validate_state`, `restore_state`（既定は状態なし） |
| `Strategy` | 必須 | `on_event` | なし |
| `TraceSink` | 任意 | `write_header`, `write` | `is_present`。登録しない場合は `Kernel::create` を使う（内部で `NoTraceSink`）。登録する場合は `Kernel::create_with_trace` |
| `SnapshotPort` | 任意 | `store` | なし。`Kernel::capture` を呼ぶときだけ渡す |

- 複数フィードを使う場合は `Kernel::create_multi` を使います。
- スナップショットの保存（`Kernel::capture`）と復元（`Kernel::restore`）を使う場合は、`EventFeedPort` の `position` が `seekable: true` を返し、`seek` と `validate_position` も実装してください。位置を戻せないフィードがあると、保存は `ERR_SNAPSHOT_UNSUPPORTED` で失敗します。`ExecutionPort` の内部状態を持つ場合は `capture_state` / `validate_state` / `restore_state` も実装します。
- 実装例は `core/tests/common/mod.rs`（`VecFeed`、`FillNextExec`、`RecordingStrategy`）と `core/benches/step_throughput.rs` を参照してください。いずれもテスト・計測用で、公開 API ではありません。
- Python の `InMemoryMarketFeed` / `SimExecution` は `bindings/python` 内の型で、`core/` からは使えません。

## ローカル検証コマンド

環境ごとの前提や詳細手順は [EK-SETUP-001](docs/setup/EK-SETUP-001_環境構築手順書.md) を参照してください。

```bash
# Rust ワークスペース（QG-001〜QG-006）
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
python3 .github/scripts/check_core_attrs.py
cargo test --workspace --locked

# FFI 契約・C ヘッダ・wheel（QG-007, QG-012, QG-013）
uv sync --extra dev --extra ffi --locked
cargo build -p event-kernel-capi --locked
uv run --no-sync maturin develop --manifest-path bindings/python/Cargo.toml --locked
uv run --no-sync pytest bindings/python/tests -v

# Python 品質（QG-010）
uv run --no-sync ruff check . && uv run --no-sync ruff format --check . && uv run --no-sync mypy . && uv run --no-sync pytest

# ドキュメント・Mermaid 検証（QG-011）
python3 .github/scripts/validate_docs.py docs README.md
node .github/scripts/validateMermaidMarkdown.mjs --all
```
