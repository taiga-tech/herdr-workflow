---
id: DOC-GLOSSARY
title: "用語と所有権"
status: draft
documentVersion: "0.2"
updated: "2026-09-11"
---

# 用語と所有権

[文書目次](README.md) · [更新ルール](maintenance.md)

> statusは`draft`です。実装済みとして扱う範囲は、コードと試験結果で確認します。

この文書の管理対象：Repository、Run、Task、Attempt、所有権の定義。

<a id="source-04"></a>

## 実行単位と所有権

| 用語           | 定義                                                                   |
| -------------- | ---------------------------------------------------------------------- |
| Repository     | 同じ共通Gitディレクトリを参照するリポジトリ                            |
| Checkout       | 実際の作業ディレクトリ。main worktreeとlinked worktreeを区別する       |
| Run            | 承認済み設定を、一つのcheckoutへ適用する実行単位                       |
| Task           | DAG上の処理。commandまたはagent                                        |
| Attempt        | Taskの一回の起動試行。再試行のたびに別IDを発行する                     |
| Resource       | プラグインが作成または明示的に採用したworkspace、tab、pane、プロセス等 |
| SessionContext | 接続先Herdrと、そのサーバー世代を識別する情報                          |

初期版は一つのRunに一つのworktreeを対応させる。Agentごとに編集先を分ける場合は、異なるworktreeを持つRunを作る。同じworktreeで複数Agentを起動しただけでは、編集ファイルは分離されない。

リソースには`owned`と`borrowed`を記録する。Runが作成したリソースのみを自動停止・削除の対象とし、既存worktreeや利用者のペインは既定で`borrowed`とする。パスが同じという理由だけでは所有権を推定しない。

## 補助用語

| 用語         | 定義・参照先                                                                     |
| ------------ | -------------------------------------------------------------------------------- |
| job          | 終了コードで成否を判定する一回実行のcommand。                                    |
| service      | foregroundで継続稼働するcommand。準備完了と終了成功を分ける。                    |
| readiness    | 起動後に依存側の処理を開始してよいかを表す条件。Taskの実行状態とは別に記録する。 |
| Coordinator  | Run受付、計画、依存判定、所有権を管理する常駐処理。                              |
| TaskRunner   | Attempt単位で子プロセスを起動・観測し、ログと結果を記録する処理。                |
| ADR          | 一つの設計判断について、背景、選択、代替案、結果、状態を記録した文書。           |
| 正式な参照先 | ある仕様を変更するときに編集する主文書。目次や機能一覧はその文書への案内とする。 |
| 実装確認     | 対象版、環境、試験結果を根拠に挙動を確かめること。文書検査の合格とは別。         |

## 関連文書

[状態モデル](specs/state-model.md) / [停止と後片付け](specs/cleanup.md)
