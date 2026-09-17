---
id: REF-CLI
title: 'CLIとHerdr action'
status: draft
documentVersion: '0.2'
updated: '2026-09-17'
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
| `schema [--output PATH]` | `WorkflowSpec`のJSON Schemaを表示またはファイルへ出力  |
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
herdr-workflow schema --output schema/workflow.schema.json

# 承認とRunの作成
herdr-workflow trust --config .herdr/workflow.yaml
herdr-workflow up --input branch=feature/example --input port=3001

# upが返した実際のRun IDを指定する
herdr-workflow status RUN_ID
herdr-workflow logs RUN_ID --task server --follow
herdr-workflow stop RUN_ID
```

`up`の受付成功は、初期化の成功を意味しない。`up`は受付記録とRun IDを返し、成否は`wait`、`status`、通知で取得する。`run --target`はserviceの存在によって待ち続けず、指定jobの結果で終了する。

`validate`と`plan`はworkflow設定より先に`$HERDR_PLUGIN_CONFIG_DIR/config.yaml`を読み、[プラグイン全体設定](configuration.md)の上限を適用する。環境変数またはファイルがない場合は組み込み既定値を使う。存在するプラグイン全体設定が不正な場合は終了コード2と`plugin-config`段階のエラーを返し、workflow設定の検証へ進まない。`schema`は`WorkflowSpec`の生成だけを行うため、プラグイン全体設定を読み込まない。

`plan`のハッシュには正規化したworkflow設定、解決済み入力、適用したプラグイン全体設定を含める。同じworkflowと入力でも、プラグイン全体設定が異なれば別のハッシュになる。

機械向け出力は`--json`で提供し、ログや進捗文を混在させない。引数解析エラーも`--json`指定時はJSON、それ以外は`エラー(cli): ...`形式で出力する。オプション値として渡した`--json`は出力形式の指定とは扱わない。`plan --json`の`plan.tasks`はTask IDをキーとし、各値に重複する`id`フィールドを持たない。終了コードの案は、0が操作成功、2が設定不正、3が承認待ち、4が実行失敗、5が接続・互換性エラー、6が状態不明または照合待ち、130が利用者の中止とする。生の子プロセス終了コードは結果オブジェクトにも保存する。

Herdr actionは`up / status / stop / validate / cleanup-review`を入口とし、実処理は同じCLIとCoordinatorへ集約する。選択式の操作は、manifestで宣言した端末画面を開いて入力を受け取る。

`herdr-plugin.toml`(Herdr v0.9.0で実機確認済み。[S1](../reference/sources.md#s1)[S2](../reference/sources.md#s2))は、1エントリごとに`id`・`title`・`command`(argv形式)・`contexts`(`global`/`workspace`/`tab`/`pane`/`selection`)を持つ`[[actions]]`でHerdr actionを宣言する。Herdr側はaction idを`<plugin_id>.<id>`へ修飾するため、manifest内のidは`.`を含まない`[A-Za-z0-9:_-]`にする。選択式の端末画面は`[[panes]]`(`id`・`title`・`command`・`placement`)で宣言し、`placement`は`overlay`(既定)/`popup`/`split`/`tab`/`zoomed`から選ぶ。manifestの`min_herdr_version`はセマンティックバージョニング文字列の必須フィールドで、Herdrは指定バージョンより古い自分自身へのlink/installを拒否する。対応下限の具体的な値は互換性試験後に決める。

## 関連文書

[設定](configuration.md) / [運用手順](../guides/operations.md) / [resumeとretry](../specs/recovery.md) / [削除仕様](../specs/cleanup.md)
