---
id: DOC-INDEX
title: "文書目次"
status: current
documentVersion: "0.2"
updated: "2026-09-11"
---

# 文書目次

[リポジトリの入口](../README.md) · [更新ルール](maintenance.md)

## 参照の入口

| 調べたいこと               | 文書                                                                           |
| -------------------------- | ------------------------------------------------------------------------------ |
| 何を作るか、どこまで作るか | [概要](overview.md)・[機能一覧](features.md)                                   |
| どう構成するか             | [アーキテクチャ](architecture.md)・[ディレクトリ構成](repository-structure.md) |
| どの規則で動くか           | 下記の仕様とリファレンス                                                       |
| なぜその設計なのか         | [ADR](decisions/README.md)                                                     |
| 実装順と未決事項           | [実装段階](planning/roadmap.md)・[未決事項](planning/open-questions.md)        |
| どう検証するか             | [受け入れ試験](testing/acceptance.md)・[開発ガイド](guides/development.md)     |

各文書の`status`と検証範囲を確認する。`draft`の仕様は、コードと試験結果で確認できる範囲だけを実装済みとして扱う。文書版と設定スキーマ版は別の番号である。

## 正式な参照先の一覧

仕様変更時は該当する主文書を編集し、目次や機能一覧に挙動の詳細を再掲しない。関係する文書は各ページの末尾から参照できる。

### 概要・構成

| 文書ID             | 文書                                        | 状態    |
| ------------------ | ------------------------------------------- | ------- |
| `DOC-OVERVIEW`     | [概要と対象範囲](overview.md)               | `draft` |
| `DOC-FEATURES`     | [機能一覧](features.md)                     | `draft` |
| `DOC-ARCHITECTURE` | [アーキテクチャ](architecture.md)           | `draft` |
| `DOC-STRUCTURE`    | [ディレクトリ構成](repository-structure.md) | `draft` |
| `DOC-GLOSSARY`     | [用語と所有権](glossary.md)                 | `draft` |

### 動作仕様

| 文書ID               | 文書                                        | 状態    |
| -------------------- | ------------------------------------------- | ------- |
| `SPEC-WORKFLOW`      | [DAGとスケジューラ](specs/workflow.md)      | `draft` |
| `SPEC-LAYOUT`        | [gridとペイン配置](specs/layout.md)         | `draft` |
| `SPEC-STATE`         | [状態モデル](specs/state-model.md)          | `draft` |
| `SPEC-EXECUTION`     | [コマンドとAgentの実行](specs/execution.md) | `draft` |
| `SPEC-OBSERVABILITY` | [readinessとログ](specs/observability.md)   | `draft` |
| `SPEC-WORKTREE`      | [worktreeとファイル操作](specs/worktree.md) | `draft` |
| `SPEC-RECOVERY`      | [イベントと復旧](specs/recovery.md)         | `draft` |
| `SPEC-PERSISTENCE`   | [状態ストア](specs/persistence.md)          | `draft` |
| `SPEC-SECURITY`      | [実行承認と秘密情報](specs/security.md)     | `draft` |
| `SPEC-CLEANUP`       | [停止と後片付け](specs/cleanup.md)          | `draft` |

### リファレンスと外部連携

| 文書ID              | 文書                                                         | 状態        |
| ------------------- | ------------------------------------------------------------ | ----------- |
| `REF-CONFIGURATION` | [設定リファレンス](reference/configuration.md)               | `draft`     |
| `REF-CLI`           | [CLIとHerdr action](reference/cli.md)                        | `draft`     |
| `REF-SOURCES`       | [外部仕様と参考資料](reference/sources.md)                   | `reference` |
| `INT-HERDR`         | [Herdr連携と表示](integrations/herdr.md)                     | `draft`     |
| `INT-EXTERNAL`      | [既存プラグインと外部ツール](integrations/external-tools.md) | `draft`     |

### 手順・計画・試験

| 文書ID              | 文書                                           | 状態    |
| ------------------- | ---------------------------------------------- | ------- |
| `GUIDE-DEVELOPMENT` | [開発環境と配布](guides/development.md)        | `draft` |
| `GUIDE-OPERATIONS`  | [運用と障害対応の手順案](guides/operations.md) | `draft` |
| `PLAN-ROADMAP`      | [実装段階](planning/roadmap.md)                | `draft` |
| `PLAN-QUESTIONS`    | [未決事項](planning/open-questions.md)         | `draft` |
| `TEST-ACCEPTANCE`   | [テストと受け入れ条件](testing/acceptance.md)  | `draft` |

### 判断・文書運用

| 文書ID            | 文書                                       | 状態      |
| ----------------- | ------------------------------------------ | --------- |
| `ADR-INDEX`       | [設計判断の記録](decisions/README.md)      | `current` |
| `DOC-MAINTENANCE` | [文書の更新と参照のルール](maintenance.md) | `current` |
| `DOC-AGENTS`      | [AI作業者向け開発案内](../AGENTS.md)       | `current` |

### 履歴・監査

以下は現行仕様ではなく、文書移行時の追跡と監査に限って参照する。

| 文書ID                | 文書                                                  | 状態       |
| --------------------- | ----------------------------------------------------- | ---------- |
| `DOC-CHANGELOG`       | [変更履歴](../CHANGELOG.md)                           | `current`  |
| `META-SOURCE-MAP`     | [元設計書からの移行表](meta/source-map.md)            | `archived` |
| `META-VALIDATION`     | [文書検査の記録](meta/validation-report.md)           | `archived` |
| `ARCHIVE-INDEX`       | [元資料の保存](../archive/README.md)                  | `archived` |
| `DOC-LEGACY-REDIRECT` | [旧設計書からの参照案内](../herdr-workflow-design.md) | `redirect` |

## 設定例と機械可読の一覧

[設定例](../examples/workflow.yaml) / [文書manifest](manifest.json)

## 更新時の確認

機能を変更するときは、機能一覧から対象仕様とT番号を確認する。判断を変えるときは新しいADRを作り、仕様・例・試験へ反映する。文書の整合検査はリポジトリルートで`python3 -B .agents/skills/herdr-workflow-docs/scripts/check_docs.py`を実行する。
