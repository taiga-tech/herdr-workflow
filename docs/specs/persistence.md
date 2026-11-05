---
id: SPEC-PERSISTENCE
title: '状態ストア'
status: draft
documentVersion: '0.2'
updated: '2026-11-05'
---

# 状態ストア

[文書目次](../README.md) · [更新ルール](../maintenance.md)

> statusは`draft`です。実装済みとして扱う範囲は、コードと試験結果で確認します。

この文書の管理対象：保存対象、書き込み経路、Runner結果の取り込み、スキーマ更新、状態ディレクトリ、制御通信、プロセスの識別。

<a id="source-15"></a>

## 状態ストア

SQLiteを状態ストアの案とし、Coordinatorの単一書き込み経路で更新する。DBはリポジトリ外のローカル状態ディレクトリへ置く。WSLでWindows側の共有パスにDBを置く構成は、初期の検証対象から外す。

| 保存対象      | 主な項目                                                      |
| ------------- | ------------------------------------------------------------- |
| `runs`        | ID、設定ハッシュ、repo、checkout、Herdr接続先、phase、outcome |
| `tasks`       | Run ID、Task ID、実行定義ハッシュ、依存条件                   |
| `attempts`    | Attempt ID、Runner情報、PIDと起動証跡、readiness、終了結果    |
| `resources`   | 種別、外部ID、所有権、作成操作、削除状態                      |
| `operations`  | 受付、開始、外部要求、結果確定、状態不明                      |
| `events`      | 発生元、Run、順序、受信時刻、正規化したイベント               |
| `approvals`   | repo、設定ハッシュ、許可内容、承認時刻、有効範囲              |
| `acceptances` | 自動受付の重複判定キー、保留理由、関連づけたRun               |

秘密値をDBへ保存しない。診断情報にも環境変数全体やコピーしたファイル内容を含めない。

TaskRunnerはDBへ直接書かず、Attempt用の結果ファイルを一時ファイルからの置換で確定する。Coordinatorが取り込む。現在の実装ではTaskRunnerをCoordinator内のスレッドとして動かしており、Coordinatorが異常終了すると結果ファイルを書けない。再起動したCoordinatorは未完了のRunを照合し、記録と一致するプロセスが稼働中なら再起動も停止もせず`unknown`とし、結果ファイルがあればその結果を、どちらもなければ`unknown`を記録する。Runは`phase=stopped, outcome=unknown`となる。結果取り込みを繰り返しても同じAttemptの結果を重複適用しない。

スキーマ版を`PRAGMA user_version`で持ち、更新前に`state.v<旧版>.bak`へバックアップを作成する。現在の版は3である(版2で`approvals`と`acceptances`、版3で`runs.owned_workspace`を追加)。旧版が読めないDBへ戻す場合はエラーを返し、未知の列や状態を黙って捨てない。

<a id="state-directory"></a>

## 状態ディレクトリ

状態ディレクトリはHerdrのプラグイン状態ディレクトリ(`HERDR_PLUGIN_STATE_DIR`)を使わず、次の順で決める。判断理由は[ADR-0007](../decisions/0007-state-directory-ipc.md)を参照する。

| 優先 | 条件                                 | 状態ディレクトリ                    |
| ---- | ------------------------------------ | ----------------------------------- |
| 1    | `HERDR_WORKFLOW_STATE_DIR`が空でない | その値(絶対パスに限る)              |
| 2    | `XDG_STATE_HOME`が空でない           | `$XDG_STATE_HOME/herdr-workflow`    |
| 3    | `HOME`が空でない                     | `$HOME/.local/state/herdr-workflow` |
| -    | いずれもない、または相対パス         | エラー。一時ディレクトリへ移さない  |

LinuxとmacOSで同じ規則を使う。Windowsネイティブの規則は段階Eで定める。同じ利用者でも環境変数が異なれば別の状態ディレクトリになるため、Herdr経由の起動とCLIからの起動で環境を揃える。Coordinatorは状態ディレクトリごとに一つとする。

```text
<state-dir>/
  coordinator.lock        Coordinatorの排他ロック(OSのファイルロック)
  coordinator.sock        制御用Unix domain socket
  state.db                SQLite
  locks/<名前>.lock       resource lockとcheckout実行ロック(OSのファイルロック)
  runs/<run-id>/attempts/<task-id>-<attempt>/
    launch.json           Runnerの起動証跡
    result.json           終了結果(一時ファイルからの置換で確定)
    stdout.log
    stderr.log
```

ディレクトリは`0700`、ファイルとソケットは`0600`で作成する。既存のディレクトリが他の利用者の所有、またはグループ・その他に書き込み権限を持つ場合は、権限を変更せずエラーにする。Herdrの`plugin uninstall`ではこのディレクトリは削除されない。

<a id="control-protocol"></a>

## 制御通信

CLI、Herdr action、event hookは`coordinator.sock`へ接続し、UTF-8のJSON Lines(1行に1つのJSONオブジェクト、改行区切り)で要求を送る。1行の上限は1 MiBとし、超えた要求は処理せずエラー応答を返して接続を閉じる。一つの接続で複数の要求を順に送ってよい。

```text
{"v":1,"id":"<要求ID>","method":"<操作名>","params":{}}
{"v":1,"id":"<要求ID>","ok":true,"result":{}}
{"v":1,"id":"<要求ID>","ok":false,"error":{"code":"<分類>","message":"<説明>"}}
```

- `v`はプロトコル版。Coordinatorが扱えない版にはエラー応答を返し、推測して解釈しない。
- `id`は要求側が付け、応答は同じ`id`を返す。受付の重複判定には使わず、重複判定は[復旧](recovery.md)の排他キーで行う。
- 変更系の要求は、受付を状態ストアへ保存した後に`ok:true`を返す。応答を受け取れなかった要求側は同じ変更を再送せず、状態を照会する。
- 操作名と各`params`は、対応するCLIサブコマンドの実装時に[CLI](../reference/cli.md)で定義する。

ソケットパスがOSの`sockaddr_un`の長さ上限を超える場合はエラーにし、別の場所へソケットを作らない。接続相手の利用者IDをOSの機能(Linuxの`SO_PEERCRED`、macOSの`getpeereid`)で確認し、Coordinatorと異なる場合は拒否する。この確認は同一利用者の任意コードからの防御を提供しない。[承認](security.md)

<a id="process-identity"></a>

## プロセスの識別

TaskRunnerは子プロセスを新しいプロセスグループの先頭として起動し、起動直後に次の値を`launch.json`へ記録する。TaskRunner自身についても同じ値を記録する。

| 項目               | Linux                             | macOS                             |
| ------------------ | --------------------------------- | --------------------------------- |
| PID                | 子プロセス起動時に得たPID         | 同左                              |
| 起動時刻           | `/proc/<pid>/stat`の`starttime`   | `sysctl(KERN_PROC_PID)`の起動時刻 |
| プロセスグループID | `getpgid`                         | 同左                              |
| ブート識別子       | `/proc/sys/kernel/random/boot_id` | `sysctl kern.boottime`            |

照合時は、記録した値と現在のOS情報を比べて次のように判定する。

| 観測結果                                      | 判定      | 扱い                                       |
| --------------------------------------------- | --------- | ------------------------------------------ |
| 4項目すべて一致                               | 同一      | 稼働中として扱い、二重起動しない           |
| PIDが存在しない、またはブート識別子が異なる   | 不在      | `result.json`を確認し、なければ`unknown`   |
| PIDは存在するが起動時刻またはグループが異なる | 不在      | PID再利用とみなし、そのPIDへ信号を送らない |
| OS情報を取得できない、記録が欠けている        | `unknown` | 自動で再起動・停止しない                   |

現在の実装はLinuxの`/proc`だけから起動時刻とブート識別子を取得する。macOSの`sysctl`による取得は未実装で、macOSでは照合結果が`unknown`になり、自動で再起動・停止しない。

停止はプロセスグループへ信号を送り、記録したグループIDと現在の値が一致する場合に限る。グループ外へ移った子孫プロセスの扱いは[後片付け](cleanup.md)で定める。

## 関連文書

[状態モデル](state-model.md) / [復旧](recovery.md) / [秘密情報](security.md)
