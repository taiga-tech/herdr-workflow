---
id: DOC-AGENTS
title: "AI作業者向け開発案内"
status: current
documentVersion: "0.2"
updated: "2026-09-12"
---

# AI作業者向け開発案内

[文書目次](docs/README.md) · [開発ガイド](docs/guides/development.md) · [更新ルール](docs/maintenance.md)

## 参照順

このリポジトリの作業では、[文書目次](docs/README.md)、[機能一覧](docs/features.md)、対象仕様、関連ADR、受け入れ試験、対象コードとテストの順に必要な範囲を読む。会話の記憶より、版管理された文書、コード、検証結果を確認する。

## 変更時の契約

このリポジトリでRust実装、テスト、設定例、文書を管理する。Rust採用は確定済みだが、`draft`の仕様と`proposed`のADRは採用済みとみなさない。実装によって契約を具体化する場合は、対象仕様、試験、未決事項、必要なADRを同じ変更で更新する。

本文、コード、設定例、試験、関連ADRの食い違いを見つけたら、その内容と正とする根拠を説明してから変更する。Herdrに存在するAPIを推測せず、対象版の仕様または実機で確認する。

既存タブの置換、借用checkoutの削除、未知の副作用の再実行を、仕様が定めた確認なしに行わない。秘密値や環境変数全体をログ・文書へ書かない。

## 編集規則

変更は必要なコード、テスト、主文書に限定する。同じ仕様を複製せず、目次で指定した主文書を更新する。文書ID、機能ID、試験ID、ADR番号を振り直さない。元資料と監査記録は`archive/`と`docs/meta/`に保存されており、現行仕様として編集しない。

設定例の編集元は`examples/workflow.yaml`。例の変更に合わせて設定型、説明、試験を確認する。開発コマンドは`mise`を入口とし、対象プロジェクトの初期化処理では`pnpm`を前提にする。

コード変更後の文書同期には[リポジトリ内スキル](.agents/skills/herdr-workflow-docs/SKILL.md)を使う。作業計画、実行ログ、レビュー過程などの一時的な記録を、README、仕様、リファレンス、ガイドへ混ぜない。

文書検査はリポジトリルートで次を実行する。

```bash
mise run docs:test
mise run docs:check
```

Rust実装・Herdr動作・実Agent起動を検査していないときは、その検査が成功したと報告しない。文書の構造検査、Rustの検査、製品の受け入れ試験は別である。

## 関連文書

[開発環境](docs/guides/development.md) / [文書更新](docs/maintenance.md) / [製品試験](docs/testing/acceptance.md)
