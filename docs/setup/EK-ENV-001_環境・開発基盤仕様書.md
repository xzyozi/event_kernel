---
title: "環境・開発基盤仕様書"
document_type: "environment_specification"
document_number: "EK-ENV-001"
document_role: "normative"
ssot_scope: "environment"
status: "draft"
version: "0.1"
created_at: "2026-11-24"
updated_at: "2026-11-24"
author: "xzyozi"
owner: "xzyozi"
purpose: "Deterministic Event Kernelの開発・検証環境、ツールチェーン、品質ゲートを定義し、設計書とテスト文書がゲートIDで参照できるようにするため"
related_documents:
  - "ドキュメントガバナンス（相対パス: ../README.md）"
  - "EK-BD-001 基本設計書（相対パス: ../design/EK-BD-001_基本設計書.md）"
  - "Deterministic Event Kernel システム要件定義書（相対パス: ../features/deterministic-event-kernel-requirements.md）"
  - "ADR-002 コア実装言語にRustを採用する（相対パス: ../adr/002-core-language-rust.md）"
  - "ADR-003 Python FFI境界方式の選定（相対パス: ../adr/003-python-ffi-boundary.md）"
  - "ADR-004 coreのメモリ確保方針（相対パス: ../adr/004-core-memory-allocation.md）"
  - "Python環境セットアップ（相対パス: ./python_setup.md）"
---

# 環境・開発基盤仕様書
**Rust コア + Python ホストの開発環境、ツールチェーン、品質ゲート**

---

## 1. 目的・対象範囲

- **目的**: コアカーネルの決定論性（NFR-01）、ホットパスの確保ゼロ（NFR-02）、性能（NFR-03）を、環境とCIで検証できる状態にする。
- **対象環境**: 開発（ローカル）、CI（GitHub Actions）。
- **対象範囲**: Rust と Python のツールチェーン、依存管理、ビルド、品質ゲート、再現性。
- **対象外**: 本番環境、運用手順、環境構築の操作手順（環境構築手順書で別途定義する）。

## 2. ハード要件・制約

- **必須プラットフォーム**: Windows と Linux。決定論性は両OSで検証する（AC-07）。
- **性能の基準機**: NFR-03 の基準機は未確定（要件定義書 OPEN-06）。確定するまで、AC-06 は参考値として記録し、合否の判定に使わない。
- **ネットワーク・アクセス**: 依存パッケージの取得に外部レジストリを使う。実行時のネットワーク通信は不要。
- **Rust の最低バージョン**: PyO3 0.30.0 の MSRV は Rust 1.85（2026-10-13 公開の変更履歴による）。これ以上のバージョンに固定する。具体的なバージョンは 4.1節で定める。
- **Python の対応範囲**: 3.10 以上（`pyproject.toml` の `requires-python`）。開発用は 3.12（`.python-version`）。

## 3. 環境概要

| 環境 | 用途 | プラットフォーム / 実行方式 | 所有者 | 更新・廃止方針 |
| :-- | :-- | :-- | :-- | :-- |
| ローカル開発 | 実装、単体テスト、設計書の作成 | Windows（開発機） | 開発者 | ツールチェーンは `rust-toolchain.toml` と `uv.lock` に従う。 |
| CI（Linux） | 全品質ゲートの実行 | GitHub Actions（`ubuntu-latest`） | xzyozi | ワークフローの更新はPRで行う。 |
| CI（Windows） | AC-07（OS間のハッシュ一致）、Windows でのビルド確認 | GitHub Actions（`windows-latest`） | xzyozi | 同上。 |

## 4. 適用する環境要素

### 4.1 言語・ランタイム・実行基盤

- **Rust**: `rust-toolchain.toml` で、チャンネルとバージョンを固定する。バージョンは 1.85 以上とし、導入時に確定する（未確定）。`rustfmt` と `clippy` を含める。
- **Python**: `uv` で管理する。`.python-version`（3.12）と `uv.lock` で固定する。
- **Node.js**: Mermaid の CI 検証（`validate-mermaid.yml`）でのみ使う。ローカルでは必須としない。

### 4.2 ツールチェーン・依存関係

| 区分 | ツール | 用途 | 備考 |
| :-- | :-- | :-- | :-- |
| Rust | `cargo`、`rustc` | ビルドとテスト | |
| Rust | `rustfmt`、`clippy` | 整形と静的解析 | `core/` には専用の lint 設定を適用する（QG-002）。 |
| Rust | `cbindgen` | `core-capi` の C ヘッダ生成 | ADR-003 |
| Rust | `maturin` | `bindings/python` の wheel 作成 | ADR-003 |
| Rust | Miri（nightly） | `ViewGuard` などのメモリ安全性の検査（V-05） | nightly を使うため、定期実行またはスパイクに限る（QG-009）。 |
| Python | `uv`、`ruff`、`mypy`、`pytest` | Python 側の品質ゲート | 既存の `ci-python.yml` を使う。 |
| 文書 | `validate_docs.py`、Mermaid 検証 | 文書の検証 | 既存の `validate-docs.yml`、`validate-mermaid.yml`。 |

**依存の方針**: `core` は外部クレートを最小にする（ADR-004 のトレードオフ）。新しい依存を追加するときは、追加の理由を PR に記載する。

### 4.3 ビルド・パッケージング・配布

- **リポジトリの構成**: Cargo ワークスペースとし、`core/`、`core-capi/`、`bindings/python/`、`adapters/` に分ける（EK-BD-001 2.1節）。禁止ルール（要件定義書 6章）は `core/` に適用する。
- **成果物**: ネイティブ戦略用のバイナリとライブラリ、C ヘッダ（`core-capi`）、Python の wheel（`bindings/python`）。
- **ビルド構成**: 性能の計測は `--release` と LTO を有効にする（要件定義書 8章）。境界を含むバイナリは `panic = "unwind"` とする（ADR-003 5.1-4）。
- **未確定**: `abi3`（安定ABI）の wheel を使うかどうかは、ADR-003 のスパイクの結果で決める。

### 4.4 CI/CD・品質ゲート

実行契機は、`main` と `develop` への push、およびこれらへの PR とする（既存の `ci-python.yml` と同じ）。Rust 用のワークフロー `ci-rust.yml` は未作成で、Rust のコードを追加する PR で作成する。

品質ゲートは次の表を正本とする。設計書とテスト文書は、ゲートIDで参照する。

| ゲートID | 検証種別 | 実行コマンドまたはワークフロー | 適用条件 | 合格基準 |
| :-- | :-- | :-- | :-- | :-- |
| QG-001 | Rust 整形 | `cargo fmt --all --check` | Rust のコードがあるとき。PR と push | 差分なし |
| QG-002 | Rust 静的解析 | `cargo clippy --workspace --all-targets --locked -- -D warnings`。`core/` には追加で、`unwrap`、`expect`、配列の添字アクセスによるパニック、`std::time::SystemTime` と `Instant`、`HashMap` と `HashSet` の使用を禁止する lint 設定を適用する | 同上 | 警告ゼロ。禁止 lint の違反ゼロ（要件定義書 6章の自動検査） |
| QG-003 | crate 属性検査 | `core/` のクレートルートに `#![no_std]` と `#![forbid(unsafe_code)]` があることを検査するスクリプト（未作成） | 同上 | 両方の属性がある。`alloc` 以外の `std` 参照がない（ADR-004） |
| QG-004 | Rust テスト | `cargo test --workspace --locked` | 同上 | 全テスト合格 |
| QG-005 | ゼロアロケーション（AC-05） | カウンタ付きアロケータを使うテスト（`cargo test` に含める） | `core/` の変更時 | 初期化完了後からループ終了までの確保・解放が0回 |
| QG-006 | 決定論性（AC-04、AC-07） | 同一入力・同一シードで単一実行と16並列実行の出力ハッシュを比較するテスト。Linux と Windows の両方で実行する | `core/` と `adapters/` の変更時。CI のマトリクス | 全出力のSHA-256が一致。OS間でも一致 |
| QG-007 | FFI 契約（V-03） | `core-capi` と `bindings/python` で同一シナリオを実行し、出力のSHA-256を比較するテスト | `core-capi` または `bindings/python` の変更時 | 完全一致 |
| QG-008 | 性能（AC-06） | 標準ベンチマーク（要件定義書 8章） | 定期実行または手動実行。PR の必須ゲートにはしない | 毎秒1,000万イベント以上。基準機の確定（OPEN-06）までは参考値として記録 |
| QG-009 | メモリ安全性（V-05） | Miri または AddressSanitizer で、入力ビューと `StrategyArena` ビューの無効化のテストを実行 | 定期実行またはスパイク。nightly を使う | 無効化後のアクセスで `RuntimeError`。メモリ破壊の検出なし |
| QG-010 | Python 静的解析・型検査・テスト | 既存の `ci-python.yml`（`ruff check`、`ruff format --check`、`mypy`、`pytest`） | 全 PR | 違反ゼロ。テスト合格 |
| QG-011 | 文書検証 | `validate-docs.yml`（`validate_docs.py`）、`validate-mermaid.yml` | `docs/` の変更時 | エラーゼロ |
| QG-012 | レビュー | PR レビュー。ホットパスでのパニック（`unwrap` など）と、`core/` への業務ロジックの混入を確認する（EK-BD-001 4.1節） | `core/` を変更する全 PR | 承認 |

**QG-005 と QG-006 の対象**: ネイティブ戦略である。Python 戦略は、GIL と Python のメモリ確保を含むため対象外とする（要件定義書 7章）。

### 4.5 設定・秘密情報

- 本システムは、秘密情報を必要としない。
- 設定は、リポジトリ内のファイル（`rust-toolchain.toml`、`Cargo.toml`、`pyproject.toml`）で管理する。
- CI のトークンなどは、GitHub のシークレットを使う。ログに出力しない。

### 4.6 再現性・サプライチェーン

- **ロックファイル**: `Cargo.lock` と `uv.lock` をコミットする。Rust は `--locked` を付けてビルドとテストを実行する。
- **ツールチェーンの固定**: `rust-toolchain.toml`、`.python-version`。
- **公開直後のパッケージの回避**: Python は `uv` のグローバル設定（`exclude-newer`）に従う。Rust には同等の標準機能がないため、依存の追加と更新は PR で内容を確認する。依存の脆弱性検査ツール（`cargo audit` など）を使うかどうかは未確定とし、依存を追加する時点で決める。
- **決定論性に関わる設定**: ビルドの構成（最適化レベル、LTO）を変えた場合も、QG-006 の出力ハッシュが一致することを確認する。

## 5. 検証方法

| 確認項目 | 確認方法 | 合格条件 | 証跡 |
| :-- | :-- | :-- | :-- |
| Rust のツールチェーンが固定されている | `rust-toolchain.toml` の存在と、CI でのバージョン出力を確認する | ファイルがあり、バージョンが1.85以上 | CI のログ |
| ゲートが実行される | PR で `ci-rust.yml` が QG-001〜QG-007 を実行する | 全ゲートが実行され、合格する | PR のチェック結果 |
| 両OSで決定論性が保たれる | QG-006 のマトリクス | Linux と Windows の出力ハッシュが一致 | CI のログ |
| ロックファイルが最新である | `cargo metadata --locked` と `uv lock --check` | 差分なし | CI のログ |

## 6. 例外・適用差異

| ID | 対象 | 標準との差異 | 理由・制約 | 影響 | 代替対応 | 解除条件 |
| :-- | :-- | :-- | :-- | :-- | :-- | :-- |
| EX-001 | ローカルの開発機 | Rust のツールチェーンが未導入 | 導入は依頼がある場合に限る | スパイク（V-01〜V-06）と Rust のゲートをローカルで実行できない | CI で実行する。または導入後にローカルで実行する | 開発機へのツールチェーン導入 |
| EX-002 | CI | `ci-rust.yml` が未作成 | Rust のコードがまだない | QG-001〜QG-009 が自動実行されない | Rust のコードを追加する最初の PR で作成する | `ci-rust.yml` の追加 |

## 7. 関連文書

- **設計書**: [EK-BD-001 基本設計書](../design/EK-BD-001_基本設計書.md)
- **要件**: [要件定義書](../features/deterministic-event-kernel-requirements.md)
- **判断理由**: [ADR-002](../adr/002-core-language-rust.md)、[ADR-003](../adr/003-python-ffi-boundary.md)、[ADR-004](../adr/004-core-memory-allocation.md)
- **Python 環境**: [python_setup.md](./python_setup.md)

## 8. 作成完了チェック

- [x] `document_role`、`ssot_scope`、`status`、`owner` を Frontmatter へ設定した。
- [x] 目的、対象範囲、対象外、ハード要件・制約を記載した。
- [x] 必要な環境要素だけを選択した。
- [x] 品質ゲートのゲートID、実行方法、適用条件、合格基準を 4.4節に記載した。
- [x] 検証方法と合格条件を記載した。
- [x] 標準から外れる点を、理由、影響、代替対応、解除条件とともに記録した。
- [ ] 環境構築手順書への参照を設定した（未作成）。
- [x] 秘密情報や利用者固有の認証情報を記載していない。

## 9. 改訂履歴 (Change Log)

| 版数 | 改訂日 | 変更者 | 変更内容・変更理由 (Why) |
| :-- | :-- | :-- | :-- |
| Rev.0.1 | 2026-11-24 | xzyozi | 初版ドラフト。BD Rev.0.4 と ADR-002、003、004 の後続作業（品質ゲートの登録）を反映した。 |
