---
id: DOC-STRUCTURE
title: "ディレクトリ構成"
status: draft
documentVersion: "0.2"
updated: "2026-09-11"
---

# ディレクトリ構成

[文書目次](README.md) · [更新ルール](maintenance.md)

> 実装前の設計案です。CLI、設定キー、既定値の動作確認は行っていません。

この文書の管理対象：実在する文書と、今後作成するRustファイルの配置。

## 配布物と実装予定の区別

この配布物に含むのは、分割したMarkdown、設定例、元資料、文書検査スクリプトである。Rustのソースコード、`Cargo.toml`、`Cargo.lock`、`mise.toml`、`herdr-plugin.toml`、JSON Schemaはまだ作成していない。

実装予定のファイルが一覧にあることを、ファイルや機能が存在する根拠にしない。

## 現在の文書構成

```text
herdr-workflow-docs/
├── README.md                       配布物の入口と導入方法
├── AGENTS.md                       AIが参照する入口と更新規則
├── CHANGELOG.md                    文書の変更履歴
├── herdr-workflow-design.md        旧名からの案内
├── docs/
│   ├── README.md                  文書目次と正式な参照先
│   ├── overview.md                目的と対象範囲
│   ├── features.md                機能ID、仕様、試験の対応
│   ├── architecture.md            構成と責務
│   ├── repository-structure.md    この文書
│   ├── glossary.md                用語と所有権
│   ├── maintenance.md             更新と参照の運用
│   ├── specs/                     動作の規則
│   ├── reference/                 設定、CLI、外部資料
│   ├── integrations/              Herdrと外部ツールの契約
│   ├── guides/                    開発と運用の手順案
│   ├── testing/                   受け入れ試験
│   ├── planning/                  実装段階と未決事項
│   ├── decisions/                 ADR
│   ├── meta/                      章の移行先と検査記録
│   └── manifest.json              文書IDとパスの一覧
├── examples/
│   └── workflow.yaml              設定例の編集元
├── archive/
│   ├── README.md                  元資料の扱い
│   ├── design-0.1.md              元の設計書、そのまま保存
│   ├── workflow-0.1.yaml          元の設定例、そのまま保存
│   └── source-manifest.json       元資料のSHA-256
└── scripts/
    └── check_docs.py               文書検査、Python標準ライブラリのみ
```

仕様の詳細ファイル名は文書目次で管理する。配置だけを調べる目的では、全本文を読み直す必要はない。

<a id="source-19"></a>

## 実装リポジトリの予定構成

単一crateから開始し、外部APIと実行管理をmoduleで分ける。文書を実装リポジトリに入れた後は、上の`docs/`等を維持して次のファイルを追加する。

```text
herdr-workflow/
├── Cargo.toml
├── Cargo.lock
├── mise.toml
├── herdr-plugin.toml
├── schema/
│   └── workflow.schema.json       Rust型から生成する予定
├── src/
│   ├── main.rs
│   ├── lib.rs
│   ├── cli.rs
│   ├── config/
│   │   ├── model.rs
│   │   ├── load.rs
│   │   └── validate.rs
│   ├── plan/
│   │   ├── graph.rs
│   │   ├── compile.rs
│   │   └── layout.rs
│   ├── runtime/
│   │   ├── coordinator.rs
│   │   ├── scheduler.rs
│   │   ├── runner.rs
│   │   ├── readiness.rs
│   │   └── cancellation.rs
│   ├── herdr/
│   │   ├── client.rs
│   │   ├── transport.rs
│   │   ├── capabilities.rs
│   │   ├── events.rs
│   │   └── types.rs
│   ├── worktree/
│   │   ├── inspect.rs
│   │   ├── create.rs
│   │   ├── adopt.rs
│   │   └── remove.rs
│   ├── files/
│   │   ├── plan.rs
│   │   ├── copy.rs
│   │   └── rollback.rs
│   ├── state/
│   │   ├── store.rs
│   │   ├── models.rs
│   │   ├── migrations.rs
│   │   └── reconcile.rs
│   ├── security/
│   │   ├── approval.rs
│   │   ├── paths.rs
│   │   └── redaction.rs
│   ├── platform/
│   │   ├── unix.rs
│   │   └── windows.rs
│   └── ui/
│       ├── status.rs
│       └── logs.rs
└── tests/
    ├── config_cases/
    ├── dag_cases/
    ├── layout_cases/
    ├── process_cases/
    ├── integration/
    └── fixtures/
```

Rust型を設定定義の基準とし、構造スキーマはそこから生成する。参照整合性、依存の成立条件、gridの分割可能性等は意味検証として別途実装する。

`HerdrClient`、`ProcessBackend`、`StateStore`の境界を用意し、Herdrを起動しなくても計画器とスケジューラを試験できるようにする。内部エラーは種類と再試行可否を持ち、文字列の一致で分岐しない。

上記は担当範囲を示す構成案。Rustのmodule宣言やvisibilityは実装時に定義する。全moduleを先に空ファイルで作成することは要求しない。

## 配置時の注意

既存リポジトリに`README.md`や`AGENTS.md`がある場合は、この配布物で置換せず、既存の内容へ文書目次へのリンクと必要な更新規則を取り込む。状態DBや実行ログ、秘密値、解決済み実行環境は文書ディレクトリへ入れない。

## 関連文書

[全ファイルの目次](README.md) / [責務](architecture.md) / [開発環境](guides/development.md)
