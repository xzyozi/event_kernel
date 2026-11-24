---
title: "coreのメモリ確保方針（初期化フェーズ限定のalloc）"
document_type: "architecture_decision_record"
document_number: "ADR-004"
document_role: "rationale"
ssot_scope: "decision_rationale"
status: "accepted"
version: "1.0"
created_at: "2026-11-24"
updated_at: "2026-11-24"
author: "xzyozi"
owner: "xzyozi"
purpose: "coreクレートのno_std構成とメモリ確保の範囲を、初期化フェーズ限定のallocに決めた理由を記録するため"
related_documents:
  - "Deterministic Event Kernel システム要件定義書（相対パス: ../features/deterministic-event-kernel-requirements.md）"
  - "ADR-002 コア実装言語にRustを採用する（相対パス: ./002-core-language-rust.md）"
  - "ADR-003 Python FFI境界方式の選定（相対パス: ./003-python-ffi-boundary.md）"
---
# ADR-004: coreのメモリ確保方針（初期化フェーズ限定のalloc）

- **ステータス**: Accepted
- **決定日**: 2026-11-24
- **決定対象**: `core` クレートのクレート属性と、メモリ確保を許可する範囲
- **関連Issue**: なし

## 1. 背景

要件定義書は、リングバッファ長と State Arena のサイズを起動時パラメータで決め（NFR-02）、確保は初期化時に行い、ホットパスでは行わない（2.1節）と定めている。一方で、`core` を `#![no_std]` かつ `#![forbid(unsafe_code)]` にして、OS時刻、OS乱数、`HashMap` の混入を構造的に防ぎたい。

実行時に決まるサイズのバッファは、`alloc` なしでは確保できない。禁止すべきなのは `alloc` の存在ではなく、ホットパスでの確保である。

## 2. 決定

**案X** を採用する。

1. `core` は `#![no_std]`、`#![forbid(unsafe_code)]` とし、`extern crate alloc` を許可する。
2. メモリ確保は初期化関数に限定する。
3. 実行時のバッファは `Box<[T]>` などの固定長の所有型で保持する。確保は要素数と同じ長さで一度に行う。`Vec<T>` はフィールドに持たず、ホットパスから `push` や `reserve` を呼べない型にする。
4. ホットパスでの確保ゼロは、カウンタ付きアロケータを使う AC-05 で検証する。
5. 1カーネルは単一スレッドで動く。ハンドルに `PhantomData<Cell<()>>` を持たせ、`Send` を許可し `Sync` を禁止する。

## 3. 理由

- `core` がバッファの所有権を完結させるので、`core-capi` と `bindings/python` は型変換と境界チェックだけで済む。ADR-003 の 5.1-8（共通ロジックを `core` に集約）と整合する。
- `Box<[T]>` で保持すれば、ホットパス側のコードが再確保を伴う API を呼べない。型で防げる範囲が広い。
- `alloc` を許可しても、`std::time` と OS 乱数は使えないままである。`no_std` には `HashMap` もない。要件定義書6章の禁止ルールの一部を、コンパイルで防げる。

## 4. 代替案

### 案Y: 呼び出し側が渡す `&mut [u8]` だけを使う（`core` は確保しない）

`core` の確保は完全にゼロになる。ただし、次の理由で採用しない。

- カーネルの型全体にライフタイム `'a` が伝播する。境界層が確保したバッファと、それを借用するカーネルを同じハンドルに格納すると、自己参照構造体になる。
- 回避のために生ポインタ管理が必要になり、`core-capi` と `bindings/python` の両方に `unsafe` とメモリ計算が重複する。ADR-003 の 5.1-8 の趣旨に反する。
- バイト列から型付き配列を安全に切り出すために、`bytemuck` などの依存が要る。

### 案Z: コンパイル時に容量を固定する（const generics）

確保自体が不要になる。ただし、NFR-02 は容量を起動時の設定パラメータで決めると定めているため、採用しない。

## 5. トレードオフ

- `#![forbid(unsafe_code)]` により、ホットパスで `unsafe` による最適化（境界検査の省略など）が使えない。安全なコードと境界検査のままで、NFR-03（毎秒1,000万イベント）を満たせるかは、実測が必要である。満たせない場合は、`unsafe` を局所化したモジュールを別 crate に分け、新しい ADR で許可を決める。
- `forbid(unsafe_code)` は `core` 自身のコードにだけ効く。依存クレートの内部の `unsafe` は対象外なので、依存は最小にする。
- 境界層が必要とする生ポインタは、`core` が公開する安全なスライスやビューから、`core-capi` が作る。

## 6. 後続作業

- [ ] 基本設計書へ、メモリ確保方針と並行性マーカーを記載する。
- [ ] 環境・開発基盤仕様書へ、`forbid(unsafe_code)` と AC-05 を品質ゲートとして登録する。
- [ ] ベンチマークで、安全なコードのままで NFR-03 を満たせるかを確認する。
