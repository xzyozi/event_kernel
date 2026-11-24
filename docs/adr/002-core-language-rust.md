---
title: "コア実装言語にRustを採用する"
document_type: "architecture_decision_record"
document_number: "ADR-002"
document_role: "rationale"
ssot_scope: "decision_rationale"
status: "accepted"
version: "1.0"
created_at: "2026-11-24"
updated_at: "2026-11-24"
author: "xzyozi"
owner: "xzyozi"
purpose: "Deterministic Event Kernelのコア実装言語をC++20ではなくRustにした理由を記録するため"
related_documents:
  - "Deterministic Event Kernel システム要件定義書（相対パス: ../features/deterministic-event-kernel-requirements.md）"
  - "ドキュメントガバナンス（相対パス: ../README.md）"
---
# ADR-002: コア実装言語にRustを採用する

- **ステータス**: Accepted
- **決定日**: 2026-11-24
- **決定対象**: Deterministic Event Kernel のコアカーネル（`core/`）の実装言語
- **関連Issue**: なし

## 1. 背景

要件定義書では、コアカーネルの実装言語を「C++20 または Rust」としていた。コアは、決定論性（NFR-01）、ホットパスでの動的確保ゼロ（NFR-02）、毎秒1,000万イベント以上の処理（NFR-03）を満たす必要がある。

## 2. 決定

コアカーネルは **Rust** で実装する。

## 3. 理由

- 未初期化メモリの読み取りやデータ競合は、安全なRustのコードでは起こらない。これらは非決定性の原因であり、要件の背景（1.1節）に直結する。
- 所有権と借用により、未来バッファへの参照公開（FR-02-2）を型レベルで制限しやすい。
- 標準ライブラリで C ABI（`extern "C"`、`#[repr(C)]`）を公開でき、Pythonとの境界（FR-06）を構築できる。
- `#![no_std]` や `#![forbid(unsafe_code)]`、Clippy の lint で、禁止ルール（要件定義書6章）の一部を機械的に検査できる。

## 4. トレードオフ

- ホットパスのパニック禁止（要件定義書6章）は、言語機能だけでは保証できない。Clippy の `unwrap_used`、`expect_used`、`indexing_slicing` の lint で検査する必要がある。
- Rustのツールチェーン（`cargo`、`rustc`）が開発環境とCIに加わる。環境の詳細は環境・開発基盤仕様書で定義する。
- Python連携のビルド（`maturin` など）が加わる。方式は未確定で、要件定義書の OPEN-02 で扱う。

## 5. 代替案

### C++20

低レベルの制御と既存資産の利用に優れる。ただし、未初期化メモリや例外送出を、規約とレビューで抑える必要がある。要件の中心である決定論性を言語側で支えにくいため、採用しない。

## 6. 後続作業

- [ ] 環境・開発基盤仕様書に、Rustのバージョン固定、`cargo` の品質ゲート、Clippy の lint 設定を記載する。
- [ ] 基本設計書で、FFI境界の方式を決める（OPEN-02）。
