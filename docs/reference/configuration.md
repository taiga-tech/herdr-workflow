---
id: REF-CONFIGURATION
title: "設定リファレンス"
status: draft
documentVersion: "0.2"
updated: "2026-09-11"
---

# 設定リファレンス

[文書目次](../README.md) · [更新ルール](../maintenance.md)

> 実装前の設計案です。CLI、設定キー、既定値の動作確認は行っていません。

この文書の管理対象：読み込み、キー表記、値置換、設定例への案内。

<a id="source-06"></a>

## 設定例

保存先の案は`.herdr/workflow.yaml`とする。参照先はNext.jsとPrismaを使うプロジェクトの例である。`mise.toml`、Prisma schema、`test:e2e`スクリプトが対象プロジェクトに存在する前提であり、プラグインが自動検出して追加するものではない。

設定全体は[examples/workflow.yaml](../../examples/workflow.yaml)を参照する。この文書には全文を複製しない。

`mise exec -- ...`を経由し、対象worktreeのmise設定で後続コマンドを実行する。`mise install`後に、親プロセスのPATHが自動更新されたと仮定しない。[S8](sources.md#s8)

この例の`PORT`は実行ごとに変更できる。並行する別Runへ同じポートを指定した場合は、利用中であることを検出して起動を止める。空きポートの検査と実際のbindの間には競合余地があるため、ポート番号の確保を検査だけで保証しない。

### 設定の規則

設定キーはcamelCaseとし、未知のキーと重複キーをエラーにする。`version`は設定スキーマの版であり、プラグインのリリース番号とは分ける。

Task IDとpane IDは明示する。表示名は識別子として使わない。Agentタスクは一つのAgent用ペインへ対応づける。ログ表示は一つのタスクに複数置いても、プロセスを再起動しない。

コマンドは`argv`を標準形式とする。引数に空白が含まれていても一つの要素として扱う。シェル構文が必要な場合だけ、実行シェルとscriptを別項目で明示する。`argv`とscriptの同時指定は拒否する。

`${inputs.port}`等はプラグインが定義した値置換であり、シェル評価ではない。未定義の変数、型不一致、パス制約違反は実行前にエラーにする。引数配列は置換後も分割しない。

YAMLの外部ファイル読み込み、任意のカスタムタグ、暗黙の実行式は初期版で扱わない。ファイルサイズ、深さ、alias展開量に上限を設定する。

### 読み込み元と承認

明示した`--config`を優先し、それ以外は作成元の`.herdr/workflow.yaml`を使う。ユーザー固有の承認と上書き設定は`HERDR_PLUGIN_CONFIG_DIR`側で管理し、承認情報をリポジトリへコミットしない。

Run開始時に設定を解決し、ハッシュ付きの実行計画として固定する。worktree作成後に別ブランチの同名設定へ自動的に読み替えない。

## 設定領域の参照先

| 領域                                    | 記載内容                         | 動作規則                              |
| --------------------------------------- | -------------------------------- | ------------------------------------- |
| `inputs`                                | 入力の型、既定値、値置換         | この文書                              |
| `worktree` / `files`                    | checkout準備と許可ファイルコピー | [worktree仕様](../specs/worktree.md)  |
| `defaults` / `tasks`                    | command、Agent、実行環境         | [実行仕様](../specs/execution.md)     |
| `bootstrap` / `dependsOn` / `execution` | 初期化対象と依存条件、実行枠     | [DAG仕様](../specs/workflow.md)       |
| `waitFor`                               | HTTP、TCP、ログによる準備確認    | [観測仕様](../specs/observability.md) |
| `workspace` / `placement`               | tab、pane、grid                  | [配置仕様](../specs/layout.md)        |
| `cleanup`                               | 停止と保存の方針                 | [後片付け仕様](../specs/cleanup.md)   |

## 仕様化の残件

設定例は入力形式の提案であり、全キーの機械可読スキーマはまだない。resource lock、秘密値参照、シェルscript、継続health check、個別retryなど、本文に動作方針があり入力形式が未定義の項目は未決事項Q11で管理する。本文にないキーを推測して追加しない。

Rustの設定型を実装した後、JSON Schemaを型から生成する。生成物と型を別々に手作業で更新しない。構造検証だけでは、循環依存やgridの分割可能性を検証したことにはならない。

## 関連文書

[設定例](../../examples/workflow.yaml) / [未決事項](../planning/open-questions.md) / [設定定義の判断](../decisions/0005-configuration-source.md)
