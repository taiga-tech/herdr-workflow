---
id: ADR-0003
title: 'ADR-0003：依存条件とreadinessの分離'
status: proposed
documentVersion: '0.2'
updated: '2026-09-11'
---

# ADR-0003：依存条件とreadinessの分離

[文書目次](../README.md) · [更新ルール](../maintenance.md)

この文書の管理対象：一つの判断の背景、選択と結果。現在の動作規則は関連仕様を参照する。

## 背景

依存先の起動、準備完了、終了成功は異なる。常駐サーバーの終了を待ってからE2Eを起動する仕様では、意図した依存関係を表せない。

## 提案する決定

依存条件を`succeeded`、`ready`、`started`として明示する。Taskの実行状態とreadiness、Agent観測状態を別に保存する。serviceのreadyをjobの成功へ変換しない。

## 比較した案

依存条件を省略して種類から推定する案、`Completed`や`Ready`を同じ列挙型に並べる案、一定時間sleepして準備完了と扱う案は、状態を取り違えやすいため採らない案とする。

## 結果と制約

意味検証と状態低下時の規則が必要になる。依存条件が成立しても実行枠やロックが取得できなければ起動しない。詳細な状態遷移と既定値は仕様を参照する。

## 採用条件

T03、T05〜T10、T16で依存条件と起動世代を検証する。仕様の提案を利用者の確定判断として記録しない。

## 関連文書

[DAG](../specs/workflow.md) / [状態](../specs/state-model.md) / [観測](../specs/observability.md)
