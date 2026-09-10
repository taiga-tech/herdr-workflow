---
id: DOC-AGENTS
title: "AI作業者向け参照案内"
status: current
documentVersion: "0.2"
updated: "2026-09-11"
---

# AI作業者向け参照案内

[文書目次](docs/README.md) · [更新ルール](docs/maintenance.md)

## 参照順

このリポジトリの作業では、[文書目次](docs/README.md)、[機能一覧](docs/features.md)、対象仕様、関連ADR、受け入れ試験の順に必要な範囲を読む。会話の記憶より、版管理された文書の状態と根拠を確認する。

## 変更時の契約

Rust採用は確定済み。他の構成・CLI・設定は実装前の設計案として扱い、未決事項を勝手に承認済みへ変更しない。本文、設定例、試験、関連ADRの食い違いを見つけたら、その内容を説明してから変更する。

既存タブの置換、借用checkoutの削除、未知の副作用の再実行を、文書が定めた確認なしに行わない。Herdrに存在するAPIを推測しない。秘密値や環境変数全体をログ・文書へ書かない。

## 編集規則

同じ仕様を複製せず、目次で指定した主文書を更新する。文書ID、機能ID、試験ID、ADR番号を振り直さない。元資料は`archive/`に保存されており、現行仕様として編集しない。

設定例の編集元は`examples/workflow.yaml`。例の変更に合わせて説明と試験を確認する。`mise`と`pnpm`を前提に記述する。文書検査は配布物ルートで次を実行する。

```bash
python3 scripts/check_docs.py
```

Rust実装・Herdr動作・実Agent起動を検査していないときは、その検査が成功したと報告しない。文書の構文検査と製品の受け入れ試験は別である。

既存リポジトリへ導入するときは、このファイルで既存`AGENTS.md`を置換せず、既存の規則と整合する形で取り込む。

## 関連文書

[文書更新](docs/maintenance.md) / [製品試験](docs/testing/acceptance.md)
