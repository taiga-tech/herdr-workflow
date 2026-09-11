---
id: DOC-ARCHITECTURE
title: "アーキテクチャ"
status: draft
documentVersion: "0.2"
updated: "2026-09-11"
---

# アーキテクチャ

[文書目次](README.md) · [更新ルール](maintenance.md)

> statusは`draft`です。実装済みとして扱う範囲は、コードと試験結果で確認します。

この文書の管理対象：構成要素、責務、依存方向、実装境界。

<a id="source-05"></a>

## 全体構成

```text
CLI / Herdr action / Herdr event
             |
       入力検証と実行受付
             |
      Coordinator（実行管理）
       |          |          |
       |          |          +-- StateStore / 実行履歴
       |          +------------- HerdrAdapter
       |                            +-- workspace / tab / pane
       |                            +-- Agent / メタデータ / 通知
       |
       +-- Planner / DAG Scheduler
                |
                +-- TaskRunner
                |     +-- 子プロセス / readiness / stdout / stderr
                |
                +-- FileOperations
                +-- WorktreeOperations

状態画面 / ログ画面 ----------> Coordinatorと状態ストアを参照
```

### Coordinator

同一ユーザーのプラグイン状態ディレクトリに対して一つ起動する。複数HerdrセッションのRunを扱うが、接続先IDを全操作に付ける。現在フォーカスされているペインへ暗黙に命令を送らない。

Coordinatorは受付キュー、実行計画、依存判定、所有権、停止要求を管理する。起動時にOSの排他ロックを取得する。単なるPIDファイルの有無では多重起動を判定しない。

### TaskRunner

コマンドごとに独立したTaskRunnerを起動し、そのTaskRunnerが実際の子プロセスを監視する。Coordinatorの再起動と子プロセスの終了判定を分けるためである。

TaskRunnerはAttempt ID、起動証跡、終了結果、ログ位置を保存する。Coordinatorが停止しても、既に動いているプロセスのログと終了結果を保存できる構成にする。ただしTaskRunner自身の異常終了時まで、プロセスの状態を確定できるとは扱わない。

### Herdr hook

hookは環境変数からイベントと対象を受け取り、Coordinatorの起動を確認して実行要求を送る。受付が永続化された後に終了する。

長い`pnpm install`や常駐サーバーをhook本体に置かない。プラグインをlinkした直後にも動くよう、各actionとevent受付がCoordinatorを起動確認する。startup hookの発火だけに依存しない。

## モジュール境界

| 境界                        | 担当                                                 | 境界外で扱うこと                               |
| --------------------------- | ---------------------------------------------------- | ---------------------------------------------- |
| Config / Planner            | 型、参照、依存関係、gridを検証し、実行計画を固定する | 外部プロセス起動やHerdrへの変更要求            |
| Coordinator / Scheduler     | 受付、実行枠、依存判定、操作記録、状態集約           | Herdrの未加工JSONを業務状態として使うこと      |
| TaskRunner / ProcessBackend | Attemptの起動、停止、readiness、出力取得             | Run全体の依存順の決定                          |
| HerdrAdapter / HerdrClient  | 通信、API型変換、接続先と実リソースの照合            | 成否未確認の変更要求の無条件再送               |
| StateStore                  | 状態と操作の永続化、結果の重複取り込み防止           | 各Runnerからの直接DB更新                       |
| CLI / UI                    | 操作受付、状態表示、確認                             | 表示ペインの開閉によるプロセスの暗黙起動・停止 |

実行順と状態遷移の規則は各仕様へ置く。この文書では責務と依存方向を管理する。Herdr、プロセス、ストレージの境界を差し替えて計画器とスケジューラを試験する。

<a id="source-appendix"></a>

## 最初に固定する設計契約

最初の実装着手点は、`WorkflowSpec`、`ExecutionPlan`、`TaskState`、`AttemptResult`の四つの型と、それを検証するテストとする。

設定例から計画を生成し、「初期化失敗時はAgentを起動しない」「serviceのready後にE2Eを実行する」「再開時に既存タスクを二重起動しない」「resumeで既存タブを置換しない」をテストで固定する。その後にHerdrAdapterを接続する。

## 設計判断の状態

Rust採用は確定済み。CoordinatorとTaskRunnerの分離、DAG条件の区別、保存済み計画からの復旧は、各ADRの状態に従う。`proposed`の判断を利用者が承認したものとして扱わない。

## 関連文書

[ディレクトリ構成](repository-structure.md) / [DAG](specs/workflow.md) / [復旧](specs/recovery.md) / [設計判断](decisions/README.md)
