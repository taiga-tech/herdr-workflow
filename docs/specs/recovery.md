---
id: SPEC-RECOVERY
title: 'イベントと復旧'
status: draft
documentVersion: '0.2'
updated: '2026-09-11'
---

# イベントと復旧

[文書目次](../README.md) · [更新ルール](../maintenance.md)

> statusは`draft`です。実装済みとして扱う範囲は、コードと試験結果で確認します。

この文書の管理対象：イベント受付、重複抑止、排他キー、クラッシュ照合、再開と再試行。

<a id="source-13"></a>

## イベント、排他制御、再開

### イベント受付

中心となるイベントは`worktree.created / worktree.opened / worktree.removed`とする。作成イベントの配送経路は対象Herdr版のCLIとTUIの両方で試験する。

イベント経路では、通知に含まれる作成済みcheckoutとworkspaceを対象にする。設定例の`worktree.mode: create`を見て別worktreeを作り直さない。内部の`@worktree.ensure`は既存checkoutの照合へ置き換え、`${inputs.branch}`はイベントのブランチ情報から解決する。ブランチ名を確定できないdetached checkoutは、自動適用せず明示的な設定を求める。

自動初期化は、ユーザー設定で許可されたリポジトリにだけ適用する。新しいworktreeを発見しただけで、任意のリポジトリの設定を実行しない。`worktree.opened`では、既存の成功済み初期化を既定で繰り返さない。

必要なHerdr版に限り、`workspace.created`等からworktree情報を照合する互換処理を用意する。フォーカス切替を初期化の一般的なトリガーにはしない。

### 自分が起こしたイベント

プラグインの`up`がworktreeを作ると、自分宛てにも作成イベントが届く。このイベントを別Runとして起動しない。

作成要求前に、repo IDとブランチに対するpending operationを記録する。応答とイベントがどちらの順に届いても、その操作へ関連づける。関連づけられない場合は、GitとHerdrの実状態を照合するまで追加起動を保留する。

### 排他キー

少なくとも以下を区別する。

```text
作成ロック       : repo ID + branch
checkout実行ロック: repo ID + canonical checkout identity
自動受付の重複判定: checkout世代 + workflow ID + 設定ハッシュ
試行識別子       : run ID + task ID + attempt ID
```

異なるHerdrセッションでも、同じcheckoutの初期化は競合させない。削除後に同じパスへ作り直されたworktreeは新しい世代として扱う。

「イベントが一度だけ届く」ことも「副作用が一度だけ起きる」ことも前提にしない。受付を記録し、重複を抑え、不明な状態を照合する方式とする。

### 起動直後のクラッシュ

プロセス起動と起動記録の保存は、一つの不可分な操作ではない。起動後・応答保存前にクラッシュした場合は、コマンドが走った可能性を残す。

復旧時はRunnerの起動証跡、結果ファイル、OSのプロセス情報を照合する。結果を確認できない場合は`unknown`とし、同じコマンドを自動的に再起動しない。稼働中のTaskRunnerが後から結果ファイルを確定した場合は、その結果を取り込む([状態ストア](persistence.md))。状態ストアには未起動(`waiting`)と記録されていても、次のAttemptの`launch.json`が残っていれば起動済みとして照合し、未起動として再実行しない([T14](../testing/acceptance.md#t14))。とくにDB更新、デプロイ、メール送信等を再実行しない。

### 再開と再試行

`resume`は保存済み計画を使って実状態を照合し、未実行であることを確認できたタスクから再開する。`retry`は新しいAttemptとして実行する。

<a id="agent-resume"></a>

Herdr連携で配置したRunは、作成したworkspace、設定のpaneとHerdrのpane ID、Agentを起動したpaneを状態ストアに記録する。`resume`と`retry`ではこの記録を使い、配置済みのtabを作り直さない([T20](../testing/acceptance.md#t20))。記録したpaneは`pane.get`で存在を確かめてから使い、見つからなければそのpane IDでは操作せず、Agentタスクを失敗にする([T17](../testing/acceptance.md#t17))。

| Agentの以前の状態                         | `resume`                                                                | `retry --task <Agent>`                      |
| ----------------------------------------- | ----------------------------------------------------------------------- | ------------------------------------------- |
| 起動していない(未起動・起動失敗)          | 記録済みのpaneで起動する                                                | 記録済みのpaneで起動する                    |
| 起動済み、記録と同じpaneで稼働中          | `agent.get`で照合して追跡を再開する。起動し直さず、プロンプトも送らない | 稼働中のAgentを止めずに要求を拒否する       |
| 起動済み、稼働していない(終了・置換)      | 起動し直さず`stopped`のままにする                                       | 記録済みのpaneで新しいAttemptとして起動する |
| 起動済み、同じ名前のAgentが別のpaneで稼働 | 追跡しない                                                              | 稼働中のAgentを止めずに要求を拒否する       |

画面を一度も配置していないRun(初期化の失敗等)では、`resume`の初期化成功後に通常どおり配置する。

初期版は、成功済みタスクを別Runへ横断して再利用するキャッシュを持たない。同じRun内で成功結果を使う場合も、設定、checkoutの同一性、出力や利用前提の変更を検査する。初期化後にlockfile等が変わった場合は成功済みという記録だけで省略しない。

<a id="resume-changes"></a>

### 再開前の変更検出

Run開始時と再開時に、checkoutの世代(ディレクトリのdevice・inode・作成時刻)、HEAD、checkout直下の依存関係とツールの定義ファイル(`pnpm-lock.yaml`・`package-lock.json`・`yarn.lock`・`bun.lock`・`bun.lockb`・`Cargo.lock`・`go.sum`・`uv.lock`・`poetry.lock`・`Gemfile.lock`・`composer.lock`・`mise.toml`・`.mise.toml`・`mise.lock`・`.tool-versions`)の内容ハッシュを状態ストアへ記録する。`resume`と`retry`は外部操作の前に記録と現在を比べる([Q15](../planning/open-questions.md#q15))。

| 変更                                     | 確認 | 再開時の扱い                                                                         |
| ---------------------------------------- | ---- | ------------------------------------------------------------------------------------ |
| checkoutが同じパスに作り直された         | 必要 | 成功済みのタスクも実行し直す                                                         |
| 監視対象ファイルの変更・追加・削除       | 必要 | 成功済みのタスクも実行し直す                                                         |
| workflow設定ファイルが記録した本文と違う | 必要 | 記録した設定で再開する。新しい設定を使うには新しいRunを開始する                      |
| 記録したHerdrのpaneが見つからない        | 必要 | paneを作り直さない。そのpaneのAgentは失敗にする([T17](../testing/acceptance.md#t17)) |
| HEADの移動                               | 不要 | 表示とイベント記録だけを行う。Agentの作業で日常的に起こるため                        |

確認が必要な変更があると、Coordinatorは何も起動せず`needs-confirmation`で一覧を返す。CLIは一覧を表示し、端末から実行していれば`y`の入力で続ける。端末がない場合や`--json`では終了コード3で終え、`--yes`での再実行を案内する。状態不明のタスクは変更の有無にかかわらず再実行しない。確認して再開した時点の状態を新しい記録とし、次の再開ではそれと比べる。検出した変更はRunのイベントに残す。

## 関連文書

[状態ストア](persistence.md) / [CLI](../reference/cli.md) / [運用手順](../guides/operations.md)
