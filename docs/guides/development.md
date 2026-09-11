---
id: GUIDE-DEVELOPMENT
title: "開発環境と配布"
status: draft
documentVersion: "0.2"
updated: "2026-09-11"
---

# 開発環境と配布

[文書目次](../README.md) · [更新ルール](../maintenance.md)

> statusは`draft`です。実装済みとして扱う範囲は、コードと試験結果で確認します。

この文書の管理対象：依存候補、miseとcargo、OS別配布、文書検査。

<a id="source-18"></a>

## 技術構成

以下は依存候補と担当範囲である。実装着手時に利用版、MSRV、license、security advisory、ビルド対象の整合を確認し、`Cargo.lock`へ固定する。この文書では未確認の「最新版番号」を記載しない。

| 担当                          | 候補                                            |
| ----------------------------- | ----------------------------------------------- |
| 非同期処理、通信、プロセス    | `tokio`                                         |
| キャンセルと入出力補助        | `tokio-util`                                    |
| 設定型、JSON                  | `serde`、`serde_json`                           |
| YAML                          | `serde-saphyr` [S6](../reference/sources.md#s6) |
| CLI引数                       | `clap`                                          |
| 依存グラフ、循環検出          | `petgraph` [S12](../reference/sources.md#s12)   |
| HTTP readiness                | `reqwest`                                       |
| ログパターン                  | `regex`                                         |
| 構造化ログ                    | `tracing`、`tracing-subscriber`                 |
| 型付きエラー、CLI最上位エラー | `thiserror`、`anyhow`                           |
| 状態ストア                    | `rusqlite` [S13](../reference/sources.md#s13)   |
| 端末画面                      | `ratatui`、`crossterm`                          |
| OS別のプロセス制御            | Unix用APIとWindows用APIを別moduleに隔離         |
| スキーマ出力                  | `schemars`等を検討。Rust型との二重管理を避ける  |

非同期処理の中でSQLiteの同期処理を長時間実行しない。専用の書き込み処理へ要求を送り、トランザクションを短く保つ。

設定ファイルの常時監視は初期版に入れず、明示的な再読み込みを使う。実行中に設定変更を検出しても、そのRunの計画を書き換えない。

<a id="source-22"></a>

## 開発環境と配布

Rustのツールチェーンは`mise.toml`に具体的な版を固定し、CIも同じ版を利用する。依存関係は`Cargo.lock`を管理対象とする。実行対象プロジェクトのNode.jsやpnpmは、そのプロジェクトのmise設定に従う。

開発チェックのコマンド案は以下とする。

```bash
mise install
mise exec -- cargo fmt --all -- --check
mise exec -- cargo clippy --locked --all-targets --all-features -- -D warnings
mise exec -- cargo test --locked
mise exec -- cargo build --locked --release
```

配布ではOSとCPU別の成果物を作り、checksumと出所を検証できる情報を公開する。開発者向けにソースからのビルド経路も残す。

Herdrの`plugin install`によるbuild処理と、配布済みバイナリの利用を分けて案内する。Herdrは不足するビルド用ツールチェーンを導入しないため、ソースビルド経路ではRustの導入を前提とする。ローカルの`plugin link`前にもビルドを行う。[S2](../reference/sources.md#s2)

単一バイナリでも、外部の`git`、`herdr`、`mise`、対象コマンドは別途必要である。またOSの動的ライブラリへの依存はビルド条件によるため、成果物ごとに検査する。

Windowsネイティブは、Herdr側でもWindows向け機能の対応状況が個別に案内されている。[S14](../reference/sources.md#s14) named pipe、Job Object、パス表現、環境変数、終了信号、PTY連携の試験が通るまで、Unix版と同じ対応済み表示をしない。WSLはLinux版として扱い、Windows上のプロセスを同時に管理する機能は含めない。

プラグイン無効化時には新しいRunの自動受付と起動を停止する。既に稼働中のRunは無断で破棄せず、`stop`で停止する。Coordinatorは接続先ごとの有効状態を定期的に照合する。アンインストール前に稼働Runと残存状態を確認できる手順を用意する。

## 検査

現在はRust実装、`Cargo.toml`、プラグインmanifestが存在しない。上記のcargoコマンドはcrateを初期化した後、このリポジトリで実行する。

文書の検査はリポジトリルートで実行する。

```bash
python3 -B .agents/skills/herdr-workflow-docs/scripts/test_check_docs.py
python3 -B .agents/skills/herdr-workflow-docs/scripts/check_docs.py
```

このテストと検査は追加Pythonパッケージを使わない。通常はMarkdownの相対リンク、明示アンカー、文書ID、受け入れ試験IDを検査する。`--check-migration`を付けた場合だけ、章の移行先と保存した元資料のハッシュも検査する。Rustのビルド、Herdr通信、設定の意味検証は対象外である。

CIでは文書検査と製品テストを別の結果として表示する。CIの権限やworkflowファイルは、対象リポジトリと必要権限を確認してから追加する。

## 関連文書

[ディレクトリ構成](../repository-structure.md) / [試験](../testing/acceptance.md) / [文書運用](../maintenance.md)
