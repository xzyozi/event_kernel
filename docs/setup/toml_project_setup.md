---
title: "プロジェクトセットアップガイド (Hatch版・旧テンプレート・廃止)"
document_type: "environment_setup_procedure"
document_role: "reference"
ssot_scope: "environment"
status: "deprecated"
version: "1.0"
created_at: "2026-10-02"
updated_at: "2026-11-27"
author: "xzyozi"
owner: "xzyozi"
purpose: "テンプレート由来の旧 Hatch 手順を記録として残し、現行の EK-SETUP-001 / python_setup.md へ誘導するため"
related_documents:
  - "ドキュメントガバナンス（相対パス: ../README.md）"
  - "EK-SETUP-001 環境構築手順書（相対パス: ./EK-SETUP-001_環境構築手順書.md）"
  - "Python プロジェクト環境構築手順書 (uv版)（相対パス: ./python_setup.md）"
---

> **非推奨（Deprecated）**: 本ファイルは初期テンプレート由来の Hatch 手順です。本プロジェクトの環境構築は [EK-SETUP-001 環境構築手順書](./EK-SETUP-001_環境構築手順書.md) および [python_setup.md](./python_setup.md) を正本としています。

# プロジェクトセットアップガイド (Hatch版)

このプロジェクトは、依存関係と開発環境の管理に [Hatch](https://hatch.pypa.io/latest/) を使用します。プロジェクトルートにある `pyproject.toml` ファイルが、従来の `setup.py` の役割を置き換えます。

## 初回のみ必要な準備

作業を始める前に、Hatchをインストールする必要があります。この作業は一度だけで結構です。

```shell
pip install hatch
```

## プロジェクトのセットアップ

プロジェクトの環境構築、すべての依存関係のインストール、Playwrightが必要とするブラウザのダウンロードを行うには、プロジェクトのルートディレクトリで以下のコマンドを実行してください。

```shell
hatch run setup
```

このコマンド一つで、以下の処理が自動的に実行されます。

1.  プロジェクト専用の仮想環境がなければ作成します。
2.  `pyproject.toml` に指定された、アプリケーション用および開発用のすべての依存関係をインストールします。
3.  `[tool.hatch.scripts]` に定義されたセットアップスクリプトを実行します。これには以下の処理が含まれます。
    *   `requirements.in` から `requirements.txt` を生成する。
    *   `requirements.txt` の内容と環境を完全に同期させる。
    *   Playwrightが必要とするブラウザドライバをインストールする。

## 仮想環境のアクティベート

プロジェクトの環境内で作業（例：スクリプトの手動実行など）を行いたい場合は、以下のコマンドで仮想環境のシェルに入ることができます。

```shell
hatch shell
```

## テストの実行

このプロジェクトでは、`pytest` を使用してテストを実行するよう設定されています。以下のコマンドでテストスイート全体を実行できます。

```shell
hatch run test
```

---
