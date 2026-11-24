# DEFAULT_PROJECT

Pythonプロジェクトの初期構成と、ドキュメント検証用のCI設定を管理するテンプレートです。

## 構成

- `pyproject.toml`: Pythonプロジェクトと開発ツールの基本設定
- `docs/`: 要求、設計、手順、検証結果などのドキュメント
- `.github/workflows/`: ドキュメントとMermaid図のCI定義
- `.github/scripts/`: CIから呼び出す検証スクリプト

## ドキュメント検証

ローカルでは次のコマンドで、`docs/` とルートREADMEを検証できます。

```powershell
python .github/scripts/validate_docs.py docs README.md
```

Mermaid図を検証する場合は、Node.jsが必要です。

```powershell
node .github/scripts/validateMermaidMarkdown.mjs --all
```
