---
id: REF-CLI
title: "CLIとHerdr action"
status: draft
documentVersion: "0.2"
updated: "2026-09-11"
---

# CLIとHerdr action

[文書目次](../README.md) · [更新ルール](../maintenance.md)

> statusは`draft`です。実装済みとして扱う範囲は、コードと試験結果で確認します。

この文書の管理対象：コマンド、結果の意味、終了コード、actionの入口。

<a id="source-20"></a>

## CLIとHerdr action

この節の`herdr-workflow`コマンドは`draft`のCLI契約であり、実装済みの範囲はコードと試験結果で確認する。

| コマンド                 | 操作                                                   |
| ------------------------ | ------------------------------------------------------ |
| `validate`               | 構文、型、参照、DAG、gridの検証。コマンドは実行しない  |
| `plan`                   | 作成、コピー、実行、配置を解決し、計画とハッシュを表示 |
| `trust`                  | 表示した設定と実行許可を承認する                       |
| `up`                     | 新規worktree用Runを受け付け、Run IDを返す              |
| `attach`                 | 明示した既存checkoutへ新しいRunを関連づける            |
| `run --target TASK`      | 対象タスクとその依存処理を実行し、対象の結果を待つ     |
| `wait RUN --until ready` | 初期化、配置、選択した常駐タスクの準備を待つ           |
| `status [RUN]`           | 実行状態、待機条件、失敗、最終確認時刻を表示           |
| `logs RUN --task TASK`   | 指定タスクのログを表示                                 |
| `stop RUN`               | 所有タスクを停止する。worktreeは残す                   |
| `resume RUN`             | 保存済み計画と実状態を照合して再開する                 |
| `retry RUN --task TASK`  | 指定タスクを新しいAttemptとして再試行する              |
| `cleanup plan RUN`       | 削除候補と保留理由を表示する                           |
| `remove RUN`             | 確認済みの所有worktreeを削除する                       |
| `doctor`                 | Herdr API、Git、mise、パス、通信、権限を検査する       |

```bash
# 構文と実行計画の確認
herdr-workflow validate --config .herdr/workflow.yaml
herdr-workflow plan --input branch=feature/example --input port=3001

# 承認とRunの作成
herdr-workflow trust --config .herdr/workflow.yaml
herdr-workflow up --input branch=feature/example --input port=3001

# upが返した実際のRun IDを指定する
herdr-workflow status RUN_ID
herdr-workflow logs RUN_ID --task server --follow
herdr-workflow stop RUN_ID
```

`up`の受付成功は、初期化の成功を意味しない。`up`は受付記録とRun IDを返し、成否は`wait`、`status`、通知で取得する。`run --target`はserviceの存在によって待ち続けず、指定jobの結果で終了する。

機械向け出力は`--json`で提供し、ログや進捗文を混在させない。終了コードの案は、0が操作成功、2が設定不正、3が承認待ち、4が実行失敗、5が接続・互換性エラー、6が状態不明または照合待ち、130が利用者の中止とする。生の子プロセス終了コードは結果オブジェクトにも保存する。

Herdr actionは`up / status / stop / validate / cleanup-review`を入口とし、実処理は同じCLIとCoordinatorへ集約する。選択式の操作は、manifestで宣言した端末画面を開いて入力を受け取る。manifestの`min_herdr_version`は互換性試験後に決める。

## 関連文書

[運用手順](../guides/operations.md) / [resumeとretry](../specs/recovery.md) / [削除仕様](../specs/cleanup.md)
