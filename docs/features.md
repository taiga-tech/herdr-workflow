---
id: DOC-FEATURES
title: '機能一覧'
status: draft
documentVersion: '0.2'
updated: '2026-09-16'
---

# 機能一覧

[文書目次](README.md) · [更新ルール](maintenance.md)

> statusは`draft`です。実装済みとして扱う範囲は、コードと試験結果で確認します。

この文書の管理対象：機能ID、対象範囲、仕様と試験への案内。

<a id="source-03"></a>

## 機能範囲

| 分野       | 対象とする機能                                                   |
| ---------- | ---------------------------------------------------------------- |
| worktree   | 新規作成、既存checkoutの採用、作成元の記録、削除前の確認         |
| 初期化     | 許可されたファイルのコピー、mise、依存パッケージ導入、コード生成 |
| DAG        | 循環検出、依存条件、並行実行、失敗伝播、期限、キャンセル         |
| レイアウト | workspace / tab / pane、grid、`colSpan`、`rowSpan`               |
| コマンド   | 一回で終了するjob、常駐service、終了コード、状態、ログ           |
| Agent      | Herdr経由の起動、準備待ち、観測状態、明示的なプロンプト送信      |
| readiness  | HTTP、TCP、ログ一致。確認期限と連続成功回数                      |
| 永続化     | 実行履歴、試行履歴、作成リソース、状態不明からの照合             |
| 操作       | CLI、Herdr action、端末内の状態画面、ログ表示                    |
| 後片付け   | 所有プロセスの停止、所有ペインの整理、明示的なworktree削除       |

自動merge、自動push、強制的なブランチ削除、利用者の既存レイアウトの無条件置換、任意コードのsandbox実行は行わない。外部コマンドが行った変更をすべて元に戻す機能も提供しない。

## 機能から仕様・試験への参照

対象段階は計画を表す。実装状態は対象コードと試験結果で確認し、表の説明は検索用の要約として使う。動作を変更するときはリンク先の主文書を編集する。試験IDがあることは実行済みを意味しない。

| ID                  | 機能             | 内容                                           | 主文書                         | 段階 | 受け入れ試験                                                                                                                                                                                                                                                                                                     |
| ------------------- | ---------------- | ---------------------------------------------- | ------------------------------ | ---- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| <a id="f01"></a>F01 | worktree         | 新規作成、既存checkoutの採用、作成元の記録     | [仕様](specs/worktree.md)      | C・D | [T13](testing/acceptance.md#t13)、[T18](testing/acceptance.md#t18)、[T28](testing/acceptance.md#t28)                                                                                                                                                                                                             |
| <a id="f02"></a>F02 | 初期化とファイル | 許可リストによるコピー、mise、pnpm、コード生成 | [仕様](specs/worktree.md)      | B・C | [T04](testing/acceptance.md#t04)、[T22](testing/acceptance.md#t22)、[T23](testing/acceptance.md#t23)                                                                                                                                                                                                             |
| <a id="f03"></a>F03 | DAG              | 循環検出、依存条件、並行実行、失敗伝播         | [仕様](specs/workflow.md)      | A・B | [T02](testing/acceptance.md#t02)、[T03](testing/acceptance.md#t03)、[T05](testing/acceptance.md#t05)、[T08](testing/acceptance.md#t08)、[T09](testing/acceptance.md#t09)                                                                                                                                         |
| <a id="f04"></a>F04 | レイアウト       | grid、span、分割木、既存ペインの保護           | [仕様](specs/layout.md)        | A・C | [T19](testing/acceptance.md#t19)、[T20](testing/acceptance.md#t20)、[T33](testing/acceptance.md#t33)                                                                                                                                                                                                             |
| <a id="f05"></a>F05 | コマンド         | job、service、終了、状態、プロセス停止         | [仕様](specs/execution.md)     | B    | [T06](testing/acceptance.md#t06)、[T14](testing/acceptance.md#t14)、[T21](testing/acceptance.md#t21)、[T26](testing/acceptance.md#t26)                                                                                                                                                                           |
| <a id="f06"></a>F06 | Agent            | 起動、準備、観測、明示プロンプト               | [仕様](specs/execution.md)     | C    | [T10](testing/acceptance.md#t10)、[T30](testing/acceptance.md#t30)                                                                                                                                                                                                                                               |
| <a id="f07"></a>F07 | readinessとログ  | HTTP、TCP、ログ一致、状態低下と記録            | [仕様](specs/observability.md) | B    | [T05](testing/acceptance.md#t05)、[T06](testing/acceptance.md#t06)、[T07](testing/acceptance.md#t07)、[T08](testing/acceptance.md#t08)、[T29](testing/acceptance.md#t29)                                                                                                                                         |
| <a id="f08"></a>F08 | 永続化と復旧     | 履歴、照合、重複抑止、世代と排他               | [仕様](specs/recovery.md)      | B・D | [T11](testing/acceptance.md#t11)、[T12](testing/acceptance.md#t12)、[T13](testing/acceptance.md#t13)、[T14](testing/acceptance.md#t14)、[T15](testing/acceptance.md#t15)、[T16](testing/acceptance.md#t16)、[T17](testing/acceptance.md#t17)、[T29](testing/acceptance.md#t29)、[T31](testing/acceptance.md#t31) |
| <a id="f09"></a>F09 | 操作と診断       | CLI、action、状態画面、ログ表示                | [仕様](reference/cli.md)       | A〜D | [T01](testing/acceptance.md#t01)、[T18](testing/acceptance.md#t18)、[T21](testing/acceptance.md#t21)、[T32](testing/acceptance.md#t32)、[T33](testing/acceptance.md#t33)                                                                                                                                         |
| <a id="f10"></a>F10 | 停止と後片付け   | 所有タスク停止、明示worktree削除               | [仕様](specs/cleanup.md)       | D    | [T26](testing/acceptance.md#t26)、[T27](testing/acceptance.md#t27)、[T28](testing/acceptance.md#t28)                                                                                                                                                                                                             |
| <a id="f11"></a>F11 | 承認と秘密情報   | 信頼範囲、再承認、秘密値の扱い                 | [仕様](specs/security.md)      | A〜D | [T22](testing/acceptance.md#t22)、[T24](testing/acceptance.md#t24)、[T25](testing/acceptance.md#t25)                                                                                                                                                                                                             |

## 後続・保留の機能

| ID                  | 機能                           | 状態                                  | 参照先                                     |
| ------------------- | ------------------------------ | ------------------------------------- | ------------------------------------------ |
| <a id="f12"></a>F12 | 対話Runner                     | 段階E、PTY契約と試験は未確定          | [実行](specs/execution.md)                 |
| <a id="f13"></a>F13 | 外部プラグインのアダプター     | 段階E、処理完了を観測できる連携に限定 | [外部連携](integrations/external-tools.md) |
| <a id="f14"></a>F14 | branch別profile                | 段階E、設定形式は未確定               | [外部連携](integrations/external-tools.md) |
| <a id="f15"></a>F15 | Windowsネイティブ              | 段階E、WSL対応とは別に検証            | [開発・配布](guides/development.md)        |
| <a id="f16"></a>F16 | 複数worktreeの親DAG            | 初期版対象外、時期未定                | [未決事項](planning/open-questions.md#q06) |
| <a id="f17"></a>F17 | Run間キャッシュ                | 初期版では提供しない                  | [未決事項](planning/open-questions.md#q07) |
| <a id="f18"></a>F18 | ネイティブサイドバーのTask一覧 | 公開APIでの実現可否は未確認           | [Herdr連携](integrations/herdr.md)         |

後続機能の受け入れ試験はQ14で管理する。既存のT01〜T32だけでこれらの対応完了を判断しない。

## 関連文書

[実装段階](planning/roadmap.md) / [試験](testing/acceptance.md) / [対象外](overview.md)
