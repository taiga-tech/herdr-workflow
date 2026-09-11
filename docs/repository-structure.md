---
id: DOC-STRUCTURE
title: "ディレクトリ構成"
status: draft
documentVersion: "0.2"
updated: "2026-09-11"
---

# ディレクトリ構成

[文書目次](README.md) · [更新ルール](maintenance.md)

> statusは`draft`です。実装済みとして扱う範囲は、コードと試験結果で確認します。

この文書の管理対象：現在のリポジトリ構成と、実装で追加するRustファイルの配置。

## 現在の状態

このリポジトリでは、製品コード、テスト、仕様、設定例、開発用スキルを同じ履歴で管理する。現在は文書、設定例、文書検査スクリプト、`mise.toml`が存在する。Rustのソースコード、`Cargo.toml`、`Cargo.lock`、`herdr-plugin.toml`、JSON Schemaはまだ存在しない。

実装予定のファイルが一覧にあることを、ファイルや機能が存在する根拠にしない。

## 現在のリポジトリ構成

```text
herdr-workflow/
├── README.md                       製品と開発の入口
├── AGENTS.md                       AIが参照する開発規則
├── CHANGELOG.md                    利用者に意味のある変更履歴
├── herdr-workflow-design.md        旧名からの案内
├── mise.toml                       開発ツールとタスク
├── .agents/skills/
│   └── herdr-workflow-docs/
│       ├── SKILL.md                文書同期の判断と手順
│       ├── agents/openai.yaml      スキルの表示情報
│       └── scripts/
│           ├── check_docs.py       文書検査、Python標準ライブラリのみ
│           └── test_check_docs.py  文書検査モードの回帰テスト
├── tasks/                          作業計画と教訓。製品文書の対象外
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
│   ├── meta/                      過去文書の移行先と監査記録
│   └── manifest.json              文書IDとパスの一覧
├── examples/
│   └── workflow.yaml              設定例の編集元
├── archive/
│   ├── README.md                  元資料の扱い
│   ├── design-0.1.md              元の設計書、そのまま保存
│   ├── workflow-0.1.yaml          元の設定例、そのまま保存
│   └── source-manifest.json       元資料のSHA-256
```

仕様の詳細ファイル名は文書目次で管理する。配置だけを調べる目的では、全本文を読み直す必要はない。

<a id="source-19"></a>

## 実装で追加する構成

単一crateから開始し、外部APIと実行管理をmoduleで分ける。現在の構成を維持して、実装の進行に応じて次のファイルを追加する。

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

状態DB、実行ログ、秘密値、生成物、一時的な作業記録を文書ディレクトリへ入れない。追跡対象と除外対象は実装時に`.gitignore`と配布仕様で明示する。

## 関連文書

[全ファイルの目次](README.md) / [責務](architecture.md) / [開発環境](guides/development.md)
