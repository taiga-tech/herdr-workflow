---
id: SPEC-STATE
title: '状態モデル'
status: draft
documentVersion: '0.2'
updated: '2026-09-17'
---

# 状態モデル

[文書目次](../README.md) · [更新ルール](../maintenance.md)

> statusは`draft`です。実装済みとして扱う範囲は、コードと試験結果で確認します。

この文書の管理対象：Task、Run、readiness、Agent観測値、Attempt世代。

<a id="source-09"></a>

## 状態モデル

### Taskの実行状態

| 状態        | 意味                                                 |
| ----------- | ---------------------------------------------------- |
| `waiting`   | 依存条件または実行枠を待っている                     |
| `starting`  | 起動要求を記録し、起動を確認している                 |
| `running`   | 対応するプロセスまたはAgentが動作中                  |
| `succeeded` | jobが終了コード0で終了した                           |
| `failed`    | 起動失敗、非0終了、準備期限超過等                    |
| `stopping`  | 停止要求を送り、終了を待っている                     |
| `stopped`   | service等を意図して停止した                          |
| `cancelled` | 開始前または実行中に明示的に中止した                 |
| `skipped`   | 依存先の失敗等により開始しなかった                   |
| `unknown`   | 起動・生存・終了のいずれかを現在の証跡で確定できない |

readinessは別フィールドで`not-applicable / pending / ready / unready`を持つ。serviceは準備が整っても`running`のままであり、`succeeded`へ変えない。

Agentの`idle / working / blocked / done / unknown`等の観測値は別フィールドに保存する。Agentの表示上の状態とTaskの終了結果を統合しない。

### Runの状態

Runも進行状態と結果を分ける。

```text
phase   = queued / preparing / active / stopping / stopped / finished
outcome = pending / succeeded / failed / cancelled / unknown
```

例えばE2Eが失敗しても、独立したサーバーやAgentを残した場合は`phase=active, outcome=failed`になる。状態画面には稼働中リソース数も表示する。

常駐serviceを含むRunは、準備完了しただけでは終了成功にならない。`ready`は待ち合わせ用の条件として扱い、`finished`と分ける。

### 起動世代

再試行ごとにAttempt IDを変える。古いAttemptのログ、readiness、終了通知によって新しいAttemptを更新しない。Attempt結果の記録では、最新と同一または古いAttempt IDの通知を無視し、履歴への重複追加と最新結果の上書きを防ぐ。

serviceの起動直後にプロセスが終了した場合は、HTTP応答が返っていても`ready`にしない。プロセスの生存、対象Attempt、readiness確認結果を同じ判定時点で照合する。

## 関連文書

[用語](../glossary.md) / [永続化](persistence.md) / [readinessとログ](observability.md)
