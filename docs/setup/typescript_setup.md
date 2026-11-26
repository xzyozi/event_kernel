---
title: "TypeScript プロジェクト環境構築手順書"
document_type: "environment_setup_procedure"
document_role: "procedure"
ssot_scope: "environment"
status: "draft"
version: "1.0"
created_at: "2026-11-04"
updated_at: "2026-11-04"
author: "xzyozi"
owner: "xzyozi"
purpose: "TypeScript 共通テンプレート（templates/typescript/）を利用した新規プロジェクト立ち上げおよび汎用 CI 連携の手順を定義するため"
related_documents:
  - "ドキュメントガバナンス（相対パス: ../README.md）"
  - "Git Flow 運用規約（相対パス: ./git_flow_and_branch_policy.md）"
---

# TypeScript プロジェクト環境構築手順書

本ドキュメントは、`DEFAULT_PROJECT` に用意された TypeScript テンプレート（`templates/typescript/`）を用いて新規 TypeScript プロジェクトを初期化し、汎用 CI（`.github/workflows/ci-typescript.yml`）を稼働させるための標準手順を定めます。

---

## 1. 目的・対象範囲

- **目的**: 統一された品質ガードレール（Biome / TypeScript / Vitest）を持つ TypeScript 開発環境の迅速な立ち上げ。
- **対象環境**: ローカル開発環境（macOS / Linux / Windows）および GitHub Actions 実行環境。
- **対象者**: プロジェクト開発者、保守担当者。
- **対象外**: Node.js 自体のインストール手順（Node.js 20 LTS 以上の利用を前提とする）。

---

## 2. 前提条件

- **ランタイム**: Node.js 20.x 以上（推奨: 最新 LTS）
- **パッケージマネージャ**: npm 10.x 以上
- **初期資材**: リポジトリ直下の `templates/typescript/` 雛形一式

---

## 3. 構築手順

### 手順 1: テンプレートの展開

リポジトリのルートディレクトリで以下のコマンドを実行し、雛形ファイルをルートに配置します。

```bash
# 雛形一式をルートに展開
cp -r templates/typescript/* .
```

### 手順 2: 不要ファイルの整理（派生プロジェクトの場合）

新規 TypeScript 専用リポジトリとして運用する場合、Python 固有の設定ファイル（`pyproject.toml` など）を削除します。

```bash
rm -f pyproject.toml
```

### 手順 3: プロジェクト名の設定

`package.json` の `name` および `description` を対象プロジェクトに合わせて更新します。

```json
{
  "name": "your-project-name",
  "version": "0.1.0",
  "description": "Project description"
}
```

### 手順 4: 依存関係のインストール

```bash
npm install
```

実行後、`package-lock.json` が生成されます。**`package-lock.json` は必ず Git の追跡対象にコミットしてください**（CI 内の `npm ci` で必須となります）。

---

## 4. 構築後の確認（ローカル検証）

以下の 4 コマンドを順に実行し、すべて正常終了（エラー 0 件）することを確認します。

| 確認項目 | 実行コマンド | 期待結果 |
| :--- | :--- | :--- |
| **Lint / Format** | `npm run lint` | 全ファイルで指摘ゼロで終了すること |
| **型検査** | `npm run typecheck` | 型エラーなしで終了すること |
| **単体テスト** | `npm test` | 0 件またはサンプルテストが通過すること |
| **ビルド** | `npm run build` | `dist/` ディレクトリにコンパイル成果物が出力されること |

---

## 5. CI/CD 連携仕様

- ルートディレクトリに `package.json` が配置されると、`.github/workflows/ci-typescript.yml` が自動的に有効化されます。
- `push` および `pull_request` イベント（対象ブランチ: `main`, `develop`）時に、上記の 4 ステップが自動検証されます。
- `package.json` が存在しない状態（テンプレートリポジトリ本体など）では、TypeScript 関連ステップが自動的にスキップされ、不要なエラーは発生しません。
