# Deterministic Event Kernel (`event_kernel`)

決定論的リプレイと先読みバイアス（Look-ahead Bias）の構造的排除を両立させる、イベント駆動カーネル基盤です。

- **リリース**: `v0.0.1`（`core/` 実装・検証済み、Python FFI 境界・アダプタ層は設計中）
- **言語構成**: Rust コア（`core/`、`#![no_std]` + `#![forbid(unsafe_code)]` + 初期化時のみ `alloc`）および Python ホスト連携基盤（設計段階）

## リポジトリ構成

- `core/` (`event-kernel-core` / `ek_core`): カーネル本体（イベントループ、K-way マージ、履歴リングバッファ、アクションキュー、Xoshiro256++ PRNG、XXH3-64 / SHA-256 トレース、スナップショット保存・復元）と機能・ゼロアロケーション・決定論性テスト・ベンチマーク
- `docs/`: 要件定義、ADR、基本/詳細/データ構造/IF 設計書、環境仕様・構築手順書、テスト計画・結果報告書（SSOT 運用は [docs/README.md](docs/README.md) を参照）
- `.github/`: Rust / Python / TypeScript / ドキュメント / Mermaid CI ワークフローと検証スクリプト

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
| テスト計画・結果 | [EK-TEST-001_テスト計画・結果報告書.md](docs/test/EK-TEST-001_テスト計画・結果報告書.md) | 検証計画・`v0.0.1` 検証実績・未実施スパイク |
| 実装計画・実測記録 | [コア縦切り実装計画.md](docs/analysis/コア縦切り実装計画.md) | `core/` 段階的実装の判断経緯とベンチマーク記録 |

## ローカル検証コマンド

環境ごとの前提や詳細手順は [EK-SETUP-001](docs/setup/EK-SETUP-001_環境構築手順書.md) を参照してください。

```bash
# Rust コア（QG-001〜QG-006）
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
python3 .github/scripts/check_core_attrs.py
cargo test --workspace --locked

# Python（QG-010）
uv run --no-sync pytest

# ドキュメント・Mermaid 検証（QG-011）
python3 .github/scripts/validate_docs.py docs README.md
node .github/scripts/validateMermaidMarkdown.mjs --all
```
