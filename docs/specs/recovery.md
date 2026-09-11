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

復旧時はRunnerの起動証跡、結果ファイル、OSのプロセス情報を照合する。結果を確認できない場合は`unknown`とし、同じコマンドを自動的に再起動しない。とくにDB更新、デプロイ、メール送信等を再実行しない。

### 再開と再試行

`resume`は保存済み計画を使って実状態を照合し、未実行であることを確認できたタスクから再開する。`retry`は新しいAttemptとして実行する。

初期版は、成功済みタスクを別Runへ横断して再利用するキャッシュを持たない。同じRun内で成功結果を使う場合も、設定、checkoutの同一性、出力や利用前提の変更を検査する。初期化後にlockfile等が変わった場合は成功済みという記録だけで省略しない。

## 関連文書

[状態ストア](persistence.md) / [CLI](../reference/cli.md) / [運用手順](../guides/operations.md)
