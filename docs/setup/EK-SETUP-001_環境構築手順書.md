---
title: "環境構築手順書"
document_type: "environment_setup_procedure"
document_number: "EK-SETUP-001"
document_role: "procedure"
ssot_scope: "environment_setup"
status: "approved"
version: "1.0"
created_at: "2026-11-26"
updated_at: "2026-11-28"
author: "xzyozi"
owner: "xzyozi"
purpose: "EK-ENV-001環境・開発基盤仕様書に沿って、Deterministic Event KernelのRust・Python・FFIビルドツールチェーンおよび文書検証環境を準備・確認・更新する手順を定義するため"
related_documents:
  - "ドキュメントガバナンス（相対パス: ../README.md）"
  - "EK-ENV-001 環境・開発基盤仕様書（相対パス: ./EK-ENV-001_環境・開発基盤仕様書.md）"
  - "EK-TEST-001 テスト計画・結果報告書（相対パス: ../test/EK-TEST-001_テスト計画・結果報告書.md）"
  - "Python環境セットアップ（相対パス: ./python_setup.md）"
  - "ドキュメント検証スクリプト運用（相対パス: ./doc_validation.md）"
  - "Mermaid CI 検証運用（相対パス: ./mermaid_ci_validation.md）"
  - "EK-BD-001 基本設計書（相対パス: ../design/EK-BD-001_基本設計書.md）"
  - "EK-DD-001 カーネルコア詳細設計書（相対パス: ../design/EK-DD-001_カーネルコア詳細設計書.md）"
  - "EK-IF-001 IF仕様書（相対パス: ../design/EK-IF-001_IF仕様書.md）"
---

# 環境構築手順書
**Deterministic Event Kernel ローカル開発・検証環境のセットアップ手順**

---

## 1. 目的・対象範囲

- **目的**: [EK-ENV-001 環境・開発基盤仕様書](./EK-ENV-001_環境・開発基盤仕様書.md) に基づき、Rust コア（`core/`）、C ABI 境界（`core-capi/`）、Python バインディング（`bindings/python/`）、およびドキュメント検証を実行できるローカル環境を再現性高く構築・確認する。
- **対象環境**: ローカル開発機（Windows / Linux、および検証用 macOS）上の開発・単体検証環境。CI 環境（GitHub Actions）の定義は `EK-ENV-001` および `.github/workflows/` を正本とする。
- **対象者**: 設計・実装・テストを担当する開発者。
- **対象外**:
  - 本番運用サーバーの構築およびデプロイ手順。
  - Python 自体の OS へのインストール手順（[Python プロジェクト環境構築手順書](./python_setup.md) に準拠）。
  - 品質ゲートの合格基準の再定義（`EK-ENV-001` 4.4節が正本）。

## 2. 前提条件・注意事項

- **参照する環境仕様**: [EK-ENV-001 環境・開発基盤仕様書](./EK-ENV-001_環境・開発基盤仕様書.md)
- **必要なアカウント・権限**: リポジトリへのクローン権限、および依存レジストリ（PyPI / crates.io）からのパッケージ取得権限。認証情報や秘密鍵は不要であり、本手順内にも記載しない。
- **必要な資材・ツール前提**:
  - Git、Python 3.10 以上（開発標準は `.python-version` の `3.12`）、`uv`。
  - Rust ツールチェーン（`rust-toolchain.toml` 指定の `1.97.1`。最小要件 MSRV `1.85` 以上。一部ローカル開発機で未導入の現状は `EK-ENV-001` 6章 `EX-001` に記載）。
  - Windows 環境でネイティブ拡張をビルドする場合は C/C++ ビルドツール（MSVC Build Tools）。Linux 環境では標準 C コンパイラ（`gcc` / `clang`）とリンカ。
  - Mermaid 図のローカル検証を行う場合のみ Node.js（CI では `22.14.0`）。
- **安全上の注意**: `uv.lock` および `Cargo.lock` を無断で破壊的更新しない。検証時は `--locked` / `--no-sync` を優先する。
- **中断条件**: ツールチェーンのバージョンが `EK-ENV-001` の最低要件（Rust 1.85 以上、Python 3.10 以上）を満たせない場合、またはハッシュ不一致・署名検証エラーが発生した場合は作業を中断し、原因を確認する。

## 3. 構築手順

Python 側の共通手順は [python_setup.md](./python_setup.md) を正本とし、本書ではプロジェクト全体の統合手順と Rust / FFI 固有手順を記載する。

| No. | 操作・設定 | 期待結果 | 異常時の対応 |
| :--- | :--- | :--- | :--- |
| 1 | **リポジトリ取得とブランチ確認**: `develop` ブランチを基準に作業ブランチを作成する | 作業ツリーがクリーンで、`pyproject.toml` と `.python-version` が存在する | ブランチや未コミット変更を整理してから再開する |
| 2 | **Python 環境構築（`uv`）**: `python_setup.md` に従い `uv sync --extra dev --extra ffi --locked` を実行する（FFI 不要の軽量検証のみなら `--extra dev`） | `.venv` が作成され、`ruff` / `mypy` / `pytest` / `maturin` / `pyarrow` / `polars` が利用可能になる | `uv` 未導入時は公式手順で導入する。Python 3.12 が取得できない場合は `uv python install 3.12` を確認する |
| 3 | **Rust ツールチェーン導入（`rustup`）**: `rustup` で Rust `1.97.1`（`rust-toolchain.toml` 指定版）および `rustfmt` / `clippy` コンポーネントを導入する | `rustc --version` が `1.97.1` を示し、`cargo fmt` / `cargo clippy` が起動できる | 導入できない場合は `EK-ENV-001` 6章 `EX-001` に従い CI 上で検証する |
| 4 | **FFI・アダプタビルド検証**: `cargo install cbindgen --locked` 導入後、C ヘッダ差分確認、C ABI 共有ライブラリビルド、`maturin build` / `develop`、FFI 契約 pytest を実行する | ヘッダ差分ゼロ（`cc` 構文検査合格）、`dist/*.whl` 生成・インポート成功、FFI/アダプタテスト全合格 | ビルドエラー時は C コンパイラ（MSVC / `cc`）および `target/debug` 上の `ek_capi` 共有ライブラリ有無を確認する |
| 5 | **Miri / ベンチ / 脆弱性検査（必要時）**: `rustup toolchain install nightly --component miri rust-src`、`cargo bench`、`cargo audit` を実行する | Miri（UB 0件）、`step_throughput` ベンチ（`NFR-03: met`）、`cargo audit` が正常終了する | 実行できない環境では GitHub Actions（`miri.yml` / `bench.yml` / `audit.yml`）で確認する |
| 6 | **文書検証準備**: Python 標準ライブラリで `validate_docs.py` を確認し、必要に応じ Node.js で `validateMermaidMarkdown.mjs` を確認する | `python3 .github/scripts/validate_docs.py docs README.md` がエラー 0 で終了する | エラー出力のファイル・行番号に従い Frontmatter またはリンクを修正する |

```bash
# 手順 2〜6 の主要確認コマンド例（Rust ツールチェーン導入後）
uv sync --extra dev --extra ffi --locked
cargo fmt --all --check && cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build -p event-kernel-capi --locked
cbindgen --config core-capi/cbindgen.toml --crate event-kernel-capi --output /tmp/event_kernel.h
diff -u core-capi/include/event_kernel.h /tmp/event_kernel.h
uv run --no-sync maturin build --manifest-path bindings/python/Cargo.toml --locked --out dist
uv run --no-sync maturin develop --manifest-path bindings/python/Cargo.toml --locked
uv run --no-sync pytest bindings/python/tests -v
uv run --no-sync python .github/scripts/validate_docs.py docs README.md
```

> [!NOTE]
> macOS 等の複数 Python が共存する環境で `.venv` を有効化せずに素の `cargo test --workspace --locked` を実行すると、システム付属の古い Python（例: Python 3.9）へリンクしようとして `bindings/python` のビルドが失敗する場合があります。その場合は `uv run --no-sync cargo test --workspace --locked` または `PYO3_PYTHON="$(pwd)/.venv/bin/python3" cargo test --workspace --locked` を使用してください。

## 4. 構築後の確認

品質ゲートの正式な定義と合格基準は [EK-ENV-001 4.4節](./EK-ENV-001_環境・開発基盤仕様書.md) を正本とする。ここではローカル環境構築直後に確認する項目のみを記載する。

| 確認項目 | 確認方法 | 合格条件 | 証跡 |
| :--- | :--- | :--- | :--- |
| Python 基盤（`QG-010`） | `uv run --no-sync ruff check .`、`ruff format --check .`、`mypy .`、`pytest` | すべてエラー 0 で終了 | ターミナル実行ログ |
| 文書検証（`QG-011`） | `python3 .github/scripts/validate_docs.py docs README.md` | エラー 0 で終了 | ターミナル実行ログ |
| Rust ツールチェーン（`QG-001`〜`QG-006`） | `rustc --version`、`cargo fmt --all --check`、`cargo clippy --workspace --all-targets --locked -- -D warnings`、`python3 .github/scripts/check_core_attrs.py`、`cargo test --workspace --locked` | Rust `1.97.1`（`rust-toolchain.toml`）かつ各検査・全テスト合格 | ターミナル実行ログ |
| FFI 成果物・寿命検証（`QG-007`・`QG-012`・`QG-013`） | `cbindgen` ヘッダ差分確認、`cargo build -p event-kernel-capi --locked`、`maturin develop --locked`、`uv run --no-sync pytest bindings/python/tests -v` | ヘッダ差分ゼロ、wheel 生成・インポート成功、FFI/アダプタ全テスト合格 | ターミナル実行ログ |
| 定期・セキュリティ補助（`QG-008`・`QG-009`・`audit`） | `cargo bench --locked -p event-kernel-core --bench step_throughput`、`cargo +nightly miri test --locked ...`、`cargo audit --deny warnings` | UB 0件、アドバイザリ違反 0件、ベンチ完走 | ターミナル実行ログ / CI |

## 5. 更新・再構築・削除（必要な場合のみ）

- **更新**: `uv.lock` 更新時は `uv sync --extra dev --extra ffi --locked` を再実行する。`rust-toolchain.toml` 更新時は `rustup show` で自動同期されたツールチェーン版数を確認する。
- **再構築**: 仮想環境の破損時は `.venv` を再同期（`uv sync --extra dev --extra ffi --reinstall`）する。Rust ビルドキャッシュ不整合時は `cargo clean` 後に `--locked` 付きで再ビルドする。
- **削除・撤去**: 環境の破棄はリポジトリ配下の `.venv`・`dist/`・`target/`・`*.egg-info/` の削除で完結する（旧テンプレート由来の `my_awesome_project.egg-info/` 等が残存している場合も、`git status` 対象外であることを確認した上で削除できる）。
- **ロールバック**: `uv.lock` や `rust-toolchain.toml` に不整合が生じた場合は、`git checkout` で直前のコミット状態へ戻して再同期する。

## 6. トラブル・例外（必要な場合のみ）

`EK-ENV-001` 6章の例外記録と整合する現状の差異を記載する。

| ID | 対象手順 | 症状・差異 | 原因・制約 | 対応 | 再発防止・解除条件 |
| :--- | :--- | :--- | :--- | :--- | :--- |
| EX-001 | 手順 3〜5 | 一部のローカル開発機に Rust ツールチェーンが未導入 | `EK-ENV-001` 6章 `EX-001` の運用方針による | 未導入機では文書・Python 検証を行い、Rust ゲートは導入済み計算機または CI（`ci-rust.yml`）で実行する | 開発機への `rustup` / `rust-toolchain.toml` 導入完了時に解除 |
| EX-002 | 手順 4〜5 | 【解除済み】4クレート（`core/`・`core-capi/`・`adapters/`・`bindings/python/`）および全 CI ゲート（`ci-rust.yml`・`bench.yml`・`miri.yml`・`audit.yml`）を整備完了 | なし | 標準手順に沿ってローカル・CI 双方で全検証を実行する | 解除済み |

## 7. 関連文書

- **環境仕様**: [EK-ENV-001 環境・開発基盤仕様書](./EK-ENV-001_環境・開発基盤仕様書.md)
- **個別手順**: [python_setup.md](./python_setup.md)、[doc_validation.md](./doc_validation.md)、[mermaid_ci_validation.md](./mermaid_ci_validation.md)、[git_flow_and_branch_policy.md](./git_flow_and_branch_policy.md)
- **設計書**: [EK-BD-001 基本設計書](../design/EK-BD-001_基本設計書.md)、[EK-DD-001 カーネルコア詳細設計書](../design/EK-DD-001_カーネルコア詳細設計書.md)、[EK-DS-001 データ構造仕様書](../design/EK-DS-001_データ構造仕様書.md)、[EK-IF-001 IF仕様書](../design/EK-IF-001_IF仕様書.md)
- **検証計画・結果**: [EK-TEST-001 テスト計画・結果報告書](../test/EK-TEST-001_テスト計画・結果報告書.md)

## 8. 作成完了チェック

- [x] `document_role`、`ssot_scope`、`status`、`owner` を Frontmatter へ設定した。
- [x] 参照する環境仕様、目的、対象環境、前提条件を記載した。
- [x] 各手順に操作内容、期待結果、異常時の対応を記載した。
- [x] 構築後の確認方法と合格条件を記載した（詳細基準は `EK-ENV-001` を参照）。
- [x] 更新・再構築・削除・ロールバックを記載した。
- [x] 標準から外れる現状の差異（`EX-001`, `EX-002`）を原因・対応・解除条件とともに記録した。
- [x] 秘密情報や利用者固有の認証情報を記載していない。

## 9. 改訂履歴 (Change Log)

| 版数    | 改訂日     | 変更者 | 変更内容・変更理由 (Why)                                                                       |
| :------ | :--------- | :----- | :--------------------------------------------------------------------------------------------- |
| Rev.0.1 | 2026-11-26 | xzyozi | 初版ドラフト。EK-ENV-001 8章の未作成項目に対応し、Rust・FFI・Python・文書検証の構築手順を整備。 |
| Rev.0.2 | 2026-11-27 | xzyozi | `Cargo.toml` / `Cargo.lock` / `rust-toolchain.toml` / `ci-rust.yml`（`QG-001`〜`QG-006`）整備済み状態および `EK-DD-001` / `EK-IF-001` / `EK-TEST-001` 参照へ同期。 |
| Rev.1.0 | 2026-11-28 | xzyozi | `--extra ffi` / `cbindgen` / `maturin` / FFIテスト / Miri / ベンチ / `cargo audit` 手順を追記し、`EX-002` 完全解除および `status` を `approved` へ更新（Issue #20）。 |
