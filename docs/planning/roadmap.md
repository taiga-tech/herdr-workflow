---
id: PLAN-ROADMAP
title: '実装段階'
status: draft
documentVersion: '0.2'
updated: '2026-09-14'
---

# 実装段階

[文書目次](../README.md) · [更新ルール](../maintenance.md)

> statusは`draft`です。実装済みとして扱う範囲は、コードと試験結果で確認します。

この文書の管理対象：実装順、各段階の出口条件、公開前の条件。

<a id="source-23"></a>

## 実装段階

| 段階          | 実装対象                                               | 次へ進む条件                                  |
| ------------- | ------------------------------------------------------ | --------------------------------------------- |
| A：計画器     | 設定型、意味検証、DAG、grid変換、plan                  | 不正設定を外部変更前に拒否できる              |
| B：非対話実行 | Runner、job/service、readiness、停止、ログ、状態ストア | サーバー準備後にE2Eを開始し、失敗を再現できる |
| C：Herdr統合  | worktree、初期化、grid配置、Agent、状態画面            | 新規worktreeからの一連の処理が動く            |
| D：復旧と運用 | 重複抑止、クラッシュ照合、再開、承認、削除確認         | 二重起動と誤削除の試験が通る                  |
| E：拡張       | 対話Runner、外部ツール連携、profile、Windowsネイティブ | 各機能の契約試験とOS別試験が通る              |

段階AとBはHerdrなしでも試験する。Herdr統合に必要なAPIの調査は並行して行い、公開前にはCとDを含む受け入れ条件を満たす。

詳細なTask一覧のネイティブサイドバー追加は、公開APIだけで扱える範囲が確認できた後、またはHerdr本体への変更案として別途進める。状態画面の提供をその判断待ちにしない。

## 進捗の記録

各段階は予定であり、完了扱いの項目はまだない。完了時は対象commit、対象OS、試験結果、未解消事項を追記する。段階Aの文書整理が済んだことと、計画器の実装完了を混同しない。

承認、排他、所有権は、初期の型定義から設計へ反映する。段階Dまで危険な削除や無承認実行を許可するという意味ではない。公開判定ではCとDの受け入れ条件を満たす。

### 段階Aの進行状況

`WorkflowSpec`/`ExecutionPlan`/`TaskState`/`AttemptResult`の型定義、未知キー・重複キー・未定義変数の検出、循環依存と依存条件の検証、gridから二分割木への変換、`execution.maxConcurrentJobs`等の並行実行設定値の意味検証(`Some(0)`の拒否)、`validate`/`plan`サブコマンドを実装した(`src/config/`・`src/plan/`・`src/state/`・`src/cli.rs`、`cargo test`93件で確認)。段階Aの出口条件のうち、これらに対応する不正設定の拒否は満たす。

`herdr-plugin.toml`(リポジトリルート)を作成し、`validate`アクションを登録した。Herdr v0.9.0の実機で`herdr plugin link`・`action invoke`により、設定不正時・妥当時の両方が正しく実行されることを確認済み(検証後unlink済み)。`plan`は`--input`必須のため引数なし呼び出しでは機能せず、対話的な`[[panes]]`実装(段階C)まで見送る。

`schemars`で`WorkflowSpec`からJSON Schemaを生成する`schema`サブコマンドを実装し、`schema/workflow.schema.json`をリポジトリへ追跡した。`jsonschema`(dev依存)で`examples/workflow.yaml`が生成スキーマに対して妥当であることと、JSON Schemaは構造検証のみを担いDAGの意味検証(循環依存等)は代替しないことを`tests/schema_contract.rs`で確認した(`cargo test`100件で確認)。

これで段階Aとして技術的に実装可能な範囲は完了した。残る未解消項目は`files.copy`の追跡済みファイル判定のみで、これは実際のworktree作成後のGit状態が必要なため、意図的に段階Bのworktree実装時へ持ち越す。段階Aを正式に完了扱いにするのはこの解消後とする。

## 関連文書

[機能一覧](../features.md) / [未決事項](open-questions.md) / [受け入れ試験](../testing/acceptance.md)
