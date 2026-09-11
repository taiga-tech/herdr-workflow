---
id: META-SOURCE-MAP
title: '元設計書からの移行表'
status: archived
documentVersion: '0.2'
updated: '2026-09-12'
---

# 元設計書からの移行表

[文書目次](../README.md) · [更新ルール](../maintenance.md)

## 移管の範囲

元文書`herdr-workflow-design.md`の27章と付録を移管した。先頭の文書属性と読み方は概要と更新ルールへ反映した。元資料は内容変更せず保存している。

全文設定例は現行のYAMLへ分離し、旧ディレクトリ図は現行文書と実装予定を区別する図に置き換えた。章番号の除去、引用資料へのリンク化、機能・試験・論点IDの追加は編集上の変更である。

## 章の対応

| 元の章                         | 移管先                                                                             | 扱い                                             |
| ------------------------------ | ---------------------------------------------------------------------------------- | ------------------------------------------------ |
| 先頭属性・読み方               | [概要](../overview.md)・[更新ルール](../maintenance.md)                            | 状態、日付の意味、対象環境を移管                 |
| 1. 目的                        | [docs/overview.md](../overview.md#source-01)                                       | 本文を移管。局所的な見出しと参照を調整           |
| 2. 確認した外部仕様            | [docs/reference/sources.md](../reference/sources.md#source-02)                     | 本文を移管。局所的な見出しと参照を調整           |
| 3. 機能範囲                    | [docs/features.md](../features.md#source-03)                                       | 範囲を維持し、機能IDと仕様・試験へのリンクを追加 |
| 4. 実行単位と所有権            | [docs/glossary.md](../glossary.md#source-04)                                       | 本文を移管。局所的な見出しと参照を調整           |
| 5. 全体構成                    | [docs/architecture.md](../architecture.md#source-05)                               | 本文を移管。局所的な見出しと参照を調整           |
| 6. 設定例                      | [docs/reference/configuration.md](../reference/configuration.md#source-06)         | 例をYAMLへ分離。規則は設定リファレンスへ         |
| 7. DAGとスケジューラ           | [docs/specs/workflow.md](../specs/workflow.md#source-07)                           | 本文を移管。局所的な見出しと参照を調整           |
| 8. gridとペイン配置            | [docs/specs/layout.md](../specs/layout.md#source-08)                               | 本文を移管。局所的な見出しと参照を調整           |
| 9. 状態モデル                  | [docs/specs/state-model.md](../specs/state-model.md#source-09)                     | 本文を移管。局所的な見出しと参照を調整           |
| 10. コマンドとAgentの実行      | [docs/specs/execution.md](../specs/execution.md#source-10)                         | 本文を移管。局所的な見出しと参照を調整           |
| 11. readinessとログ            | [docs/specs/observability.md](../specs/observability.md#source-11)                 | 本文を移管。局所的な見出しと参照を調整           |
| 12. worktreeとファイル操作     | [docs/specs/worktree.md](../specs/worktree.md#source-12)                           | 本文を移管。局所的な見出しと参照を調整           |
| 13. イベント、排他制御、再開   | [docs/specs/recovery.md](../specs/recovery.md#source-13)                           | 本文を移管。局所的な見出しと参照を調整           |
| 14. Herdr連携と表示            | [docs/integrations/herdr.md](../integrations/herdr.md#source-14)                   | 本文を移管。局所的な見出しと参照を調整           |
| 15. 状態ストア                 | [docs/specs/persistence.md](../specs/persistence.md#source-15)                     | 本文を移管。局所的な見出しと参照を調整           |
| 16. 実行承認と秘密情報         | [docs/specs/security.md](../specs/security.md#source-16)                           | 本文を移管。局所的な見出しと参照を調整           |
| 17. 停止と後片付け             | [docs/specs/cleanup.md](../specs/cleanup.md#source-17)                             | 本文を移管。局所的な見出しと参照を調整           |
| 18. 技術構成                   | [docs/guides/development.md](../guides/development.md#source-18)                   | 本文を移管。局所的な見出しと参照を調整           |
| 19. リポジトリ構成             | [docs/repository-structure.md](../repository-structure.md#source-19)               | module案を維持し、文書構成を更新                 |
| 20. CLIとHerdr action          | [docs/reference/cli.md](../reference/cli.md#source-20)                             | 本文を移管。局所的な見出しと参照を調整           |
| 21. 既存プラグインと外部ツール | [docs/integrations/external-tools.md](../integrations/external-tools.md#source-21) | 本文を移管。局所的な見出しと参照を調整           |
| 22. 開発環境と配布             | [docs/guides/development.md](../guides/development.md#source-22)                   | 本文を移管。局所的な見出しと参照を調整           |
| 23. 実装段階                   | [docs/planning/roadmap.md](../planning/roadmap.md#source-23)                       | 本文を移管。局所的な見出しと参照を調整           |
| 24. テストと受け入れ条件       | [docs/testing/acceptance.md](../testing/acceptance.md#source-24)                   | T01〜T32と期待結果を維持                         |
| 25. 設計上の保留事項           | [docs/planning/open-questions.md](../planning/open-questions.md#source-25)         | 元の10論点をQ01〜Q10とし、Q11〜Q15を追加         |
| 26. これまでの案から変更した点 | [CHANGELOG.md](../../CHANGELOG.md#source-26)                                       | 文書履歴へ移管                                   |
| 27. 参考資料                   | [docs/reference/sources.md](../reference/sources.md#source-27)                     | S1〜S14を維持し、確認範囲を明記                  |
| 付録                           | [アーキテクチャ](../architecture.md#source-appendix)                               | 最初の設計契約を移管                             |

## 今回追加した文書

文書目次、更新ルール、ADR、運用手順案、AI作業者向け案内、文書検査を追加した。新しい製品機能の承認ではない。新たな仕様判断が必要な項目は未決事項へ記録した。

## 検査可能な記録

機械可読の対応表は[migration.json](migration.json)、元資料のハッシュは[保存記録](../../archive/source-manifest.json)。検査スクリプトは27章と付録の移管先、およびアンカーの存在を検査する。

元資料のハッシュと移管先の存在だけで、意味が同一であることを証明したとは扱わない。移管時の一回限りの検査結果は現行文書として保持せず、必要な場合はGit履歴で確認する。

## 関連文書

[元資料](../../archive/README.md)
