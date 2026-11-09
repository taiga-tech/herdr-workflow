---
id: ADR-0002
title: 'ADR-0002：CoordinatorとTaskRunnerの分離'
status: accepted
documentVersion: '0.2'
updated: '2026-11-06'
---

# ADR-0002：CoordinatorとTaskRunnerの分離

[文書目次](../README.md) · [更新ルール](../maintenance.md)

この文書の管理対象：一つの判断の背景、選択と結果。現在の動作規則は関連仕様を参照する。

## 背景

イベントhookの中で初期化、長時間実行、再開をすべて処理すると、hookの終了とタスクの終了が結びつく。Coordinatorの再起動時にも、動作中タスクの証跡が必要になる。

## 決定

hookを受付に限定し、Runの制御をCoordinatorへ、Attemptの子プロセス監視をTaskRunnerへ分ける。SQLiteへの書き込みはCoordinatorの経路に限定する。

## 比較した案

hookから直接すべての処理を実行する案、Coordinatorだけで子プロセスを保持する案を検討した。どちらも採用済みとはせず、異常終了時の証跡を比較する。

## 結果と制約

IPC、プロセス起動記録、結果ファイル、再接続が必要になる。TaskRunner自体の異常終了まで状態確定を保証しない。不明なAttemptは自動再起動せず照合する。

## 採用時の具体化

利用者の判断により採用した。TaskRunnerはAttemptごとに`herdr-workflow runner`として起動する、独立したプロセスグループのプロセスとする。

- Coordinatorは起動要求(実行するコマンド、作業ディレクトリ、秘密値を解決済みの環境変数、マスク対象、ログの上限)を、TaskRunnerの標準入力へ1行のJSONで渡す。秘密値はパイプで渡すだけで、ファイルへ書かない。
- TaskRunnerは子プロセスを新しいプロセスグループで起動し、`launch.json`を書き、ログを取り込み、終了を待って`result.json`を一時ファイルからの置換で確定する。進捗(起動、readiness用のログ行、stream終了、終了)は標準出力へJSON Linesで報告する。
- 停止信号はCoordinatorがTaskRunnerの標準入力へ要求し、TaskRunnerが子プロセスの回収前に限ってグループへ送る。停止時の子孫の記録と残存分の停止もTaskRunnerが行う。
- Coordinatorが終了して標準入力が閉じても、TaskRunnerは子プロセスを止めず、ログの取り込みと`result.json`の確定を続ける。再起動したCoordinatorは稼働中のTaskRunnerを再起動せず、`result.json`が確定したら結果を取り込む。

手順と判定表は[状態ストア](../specs/persistence.md)と[復旧](../specs/recovery.md)を参照する。

## 採用条件

T14、T15、T16、T26、T29の契約試験と、未決事項Q12の整理を行う。T14、T15、T16、T26の動作は実プロセスの試験で確認する。T29の状態ストア書き込み失敗は単体試験で確認する。Herdr実機での確認は別に行う。

Q12は[ADR-0007](0007-state-directory-ipc.md)で整理した。状態ディレクトリ、制御通信、プロセスの識別は[状態ストア](../specs/persistence.md)で定め、この案の採否に関わらず適用する。残る採用条件は上記の契約試験と実機確認である。

## 関連文書

[構成](../architecture.md) / [保存](../specs/persistence.md) / [試験](../testing/acceptance.md)
