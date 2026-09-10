---
id: ADR-INDEX
title: "設計判断の記録"
status: current
documentVersion: "0.2"
updated: "2026-09-11"
---

# 設計判断の記録

[文書目次](../README.md) · [更新ルール](../maintenance.md)

## 判断の一覧

Rust採用だけを`accepted`とし、残りは元設計から取り出した`proposed`として記録する。利用者の承認を推定しない。ここには実装済みかどうかを記録するのではなく、何を採用したか・検討しているかを記録する。

| ID                                                | 判断                          | 状態       |
| ------------------------------------------------- | ----------------------------- | ---------- |
| [ADR-0001](0001-rust.md)                          | Rustの採用                    | `accepted` |
| [ADR-0002](0002-coordinator-runner.md)            | CoordinatorとTaskRunnerの分離 | `proposed` |
| [ADR-0003](0003-dependency-readiness.md)          | 依存条件とreadinessの分離     | `proposed` |
| [ADR-0004](0004-owned-resource-reconciliation.md) | 所有権と照合による復旧        | `proposed` |
| [ADR-0005](0005-configuration-source.md)          | 設定定義と生成スキーマの管理  | `proposed` |

## 新しい判断の記録

一つの判断に一つの連番ファイルを作る。背景、決定または提案、比較した案、結果と制約、確認記録を記載し、対象仕様と試験へリンクする。連番は欠番があっても再利用しない。

採用後に方針を変えるときは旧ADRを消さず、新ADRへの置換先を追記する。状態の変更と仕様の更新を同じ変更で行う。記録形式はMichael NygardのADRを参照している。[D2](../reference/sources.md#d2)

## 関連文書

[更新ルール](../maintenance.md) / [未決事項](../planning/open-questions.md)
