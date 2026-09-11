---
id: ADR-0002
title: "ADR-0002：CoordinatorとTaskRunnerの分離"
status: proposed
documentVersion: "0.2"
updated: "2026-09-11"
---

# ADR-0002：CoordinatorとTaskRunnerの分離

[文書目次](../README.md) · [更新ルール](../maintenance.md)

この文書の管理対象：一つの判断の背景、選択と結果。現在の動作規則は関連仕様を参照する。

## 背景

イベントhookの中で初期化、長時間実行、再開をすべて処理すると、hookの終了とタスクの終了が結びつく。Coordinatorの再起動時にも、動作中タスクの証跡が必要になる。

## 提案する決定

hookを受付に限定し、Runの制御をCoordinatorへ、Attemptの子プロセス監視をTaskRunnerへ分ける。SQLiteへの書き込みはCoordinatorの経路に限定する。

## 比較した案

hookから直接すべての処理を実行する案、Coordinatorだけで子プロセスを保持する案を検討した。どちらも採用済みとはせず、異常終了時の証跡を比較する。

## 結果と制約

IPC、プロセス起動記録、結果ファイル、再接続が必要になる。TaskRunner自体の異常終了まで状態確定を保証しない。不明なAttemptは自動再起動せず照合する。

## 採用条件

T14、T15、T16、T26、T29の契約試験と、未決事項Q12の整理を行う。現在は`proposed`であり、採用と実機確認が必要である。

## 関連文書

[構成](../architecture.md) / [保存](../specs/persistence.md) / [試験](../testing/acceptance.md)
