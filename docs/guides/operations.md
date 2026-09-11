---
id: GUIDE-OPERATIONS
title: "運用と障害対応の手順案"
status: draft
documentVersion: "0.2"
updated: "2026-09-11"
---

# 運用と障害対応の手順案

[文書目次](../README.md) · [更新ルール](../maintenance.md)

> statusは`draft`です。実装済みとして扱う範囲は、コードと試験結果で確認します。

この文書の管理対象：作成、確認、復旧、停止、削除前確認の順序。

## 適用条件

以下は未実装CLIに対応する運用手順案である。実装後、Herdr版とOSごとの試験結果を記載してから実運用へ適用する。現在のHerdrへこのコマンドを渡す手順ではない。

## 新しいworktreeの準備

設定を検証し、実行計画に表示されたコピー対象、コマンド、対象ブランチ、ポート、Agent起動を確認する。承認済みの内容からRunを作成する。

```bash
herdr-workflow validate --config .herdr/workflow.yaml
herdr-workflow plan --input branch=feature/example --input port=3001
herdr-workflow trust --config .herdr/workflow.yaml
herdr-workflow up --input branch=feature/example --input port=3001
```

`up`が返すRun IDは受付記録を指す。初期化の成功判定は`wait`または`status`で行う。初期化失敗時は、残されたworktreeとログを確認し、原因を直してから再試行する。

```bash
herdr-workflow status RUN_ID
herdr-workflow logs RUN_ID --task dependencies --follow
herdr-workflow wait RUN_ID --until ready
```

## 状態不明からの復旧

`unknown`を発見したときは、同じ`up`やコマンドを繰り返さない。Run ID、Attempt ID、最終確認時刻、Herdrの接続先、Runnerの証跡を確認する。

`resume`は保存済み計画と実状態を照合する操作、`retry`は新しいAttemptを作る操作である。処理済みかどうか不明なDB更新や外部送信を、再試行で解消しようとしない。

```bash
herdr-workflow doctor
herdr-workflow status RUN_ID
herdr-workflow resume RUN_ID
```

未実行を確認できないタスクは保留し、判断に必要な証跡を残す。既存ペインに変更がある場合は差分を確認し、タブの置換を復旧手段の既定にしない。

## 停止と削除確認

通常は`stop`で所有タスクを停止し、worktreeとログを残す。削除候補の確認は別の操作とする。

```bash
herdr-workflow stop RUN_ID
herdr-workflow cleanup plan RUN_ID
```

削除を行うときは`remove`の対象一覧を確認する。dirty、untrackedに加えてignoredなファイルも確認し、借用したcheckoutやmain worktreeを自動削除しない。ブランチ削除とworktree削除を同じ操作と解釈しない。

## 症状からの参照

| 状況                             | 確認する内容                                    | 仕様                                                                       |
| -------------------------------- | ----------------------------------------------- | -------------------------------------------------------------------------- |
| Agentが初期化より先に起動する    | bootstrap対象、暗黙ノード、他プラグインとの競合 | [DAG](../specs/workflow.md)・[外部連携](../integrations/external-tools.md) |
| 別のサーバーをreadyと誤認する    | ポート、Attempt、生存情報、応答の識別           | [観測](../specs/observability.md)                                          |
| 通知が重複して初期化される       | pending operation、checkout世代、排他キー       | [復旧](../specs/recovery.md)                                               |
| ログ画面を閉じてもコマンドが残る | ログ表示と停止操作の区別                        | [実行](../specs/execution.md)                                              |
| ログ・DBの書き込みに失敗する     | 容量、権限、劣化記録、新規起動の停止            | [観測](../specs/observability.md)・[永続化](../specs/persistence.md)       |
| 自動初期化が保留される           | 設定変更、承認範囲、`needs-approval`            | [承認](../specs/security.md)                                               |

操作ログを問い合わせやPRへ添えるときは、秘密値やファイル内容、環境変数全体を含めない。

## 関連文書

[CLIの契約](../reference/cli.md) / [復旧規則](../specs/recovery.md) / [削除規則](../specs/cleanup.md)
