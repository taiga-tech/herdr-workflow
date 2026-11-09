---
id: DOC-ROOT
title: 'Herdr Workflow'
status: current
documentVersion: '0.2'
updated: '2026-11-06'
---

# Herdr Workflow

[文書目次](docs/README.md) · [更新ルール](docs/maintenance.md)

## 概要

Herdrで、worktreeの準備、開発環境の初期化、タスクの依存関係、画面配置、コマンドとAgentの実行状態を一つのworkflowとして管理するプラグインを開発するリポジトリ。

実装言語はRust。設定の検証と計画(`validate`・`plan`)、Herdrなしの非対話実行(`run`・`up`・`status`・`logs`・`stop`等)、Herdr連携(`up --herdr`、worktreeイベントの受付)、対話コマンド(`runner: pty`と`term`)、branch別の`profiles`、他のHerdrプラグインのactionを呼ぶ`herdrAction`を実装している。Herdr実機での受け入れ試験はまだ行っていない。コマンドは[CLIとHerdr action](docs/reference/cli.md#implemented)、進み具合は[実装段階](docs/planning/roadmap.md)を参照する。仕様やADRの採用状態は各文書の`status`で確認し、実装済みの範囲はコードと試験結果を根拠に判断する。

## 開発の入口

[文書目次](docs/README.md)から、[機能一覧](docs/features.md)、[アーキテクチャ](docs/architecture.md)、[実装段階](docs/planning/roadmap.md)、[開発ガイド](docs/guides/development.md)を参照する。実装はこのリポジトリに追加し、仕様、コード、設定例、試験を同じ変更単位で管理する。

設定例の編集元は[examples/workflow.yaml](examples/workflow.yaml)。コード変更に伴う文書同期では[Herdr Workflow文書更新スキル](.agents/skills/herdr-workflow-docs/SKILL.md)を使う。

## リポジトリ構成

- `src/`: Rustの実装(設定、計画、実行、Coordinator、Herdr連携)
- `tests/`: CLIと実行の契約試験
- `schema/`: 設定型から生成したJSON Schema
- `herdr-plugin.toml`: Herdrプラグインのmanifest
- `docs/`: 現在の仕様、リファレンス、ガイド、ADR、受け入れ条件
- `examples/`: workflow設定例とプラグイン全体設定例
- `.agents/skills/`: このリポジトリ固有の作業スキル
- `archive/`と`docs/meta/`: 過去資料と監査記録。現行仕様としては参照しない

実装予定を含む詳しい配置は[ディレクトリ構成](docs/repository-structure.md)を参照する。

## 検査

```bash
mise run rust:fmt
mise run rust:clippy
mise run rust:test
mise run docs:test
mise run docs:check
```

Rustの試験は実際の子プロセスを起動するため、`/bin/sh`、`git`、`python3`を必要とする。

文書検査は、`mise.toml`で固定したuvとPython標準ライブラリだけで動作し、文書の構造と参照整合性を検査する。製品の動作、Rustのコンパイル、Herdr APIへの接続は文書検査の対象外である。

Rust実装の検査方法は[開発ガイド](docs/guides/development.md)、文書更新の規則は[文書更新ルール](docs/maintenance.md)、AIによる作業の入口は[AGENTS.md](AGENTS.md)を参照する。
