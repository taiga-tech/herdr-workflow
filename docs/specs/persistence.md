---
id: SPEC-PERSISTENCE
title: '状態ストア'
status: draft
documentVersion: '0.2'
updated: '2026-09-11'
---

# 状態ストア

[文書目次](../README.md) · [更新ルール](../maintenance.md)

> statusは`draft`です。実装済みとして扱う範囲は、コードと試験結果で確認します。

この文書の管理対象：保存対象、書き込み経路、Runner結果の取り込み、スキーマ更新。

<a id="source-15"></a>

## 状態ストア

SQLiteを状態ストアの案とし、Coordinatorの単一書き込み経路で更新する。DBはリポジトリ外のローカル状態ディレクトリへ置く。WSLでWindows側の共有パスにDBを置く構成は、初期の検証対象から外す。

| 保存対象     | 主な項目                                                      |
| ------------ | ------------------------------------------------------------- |
| `runs`       | ID、設定ハッシュ、repo、checkout、Herdr接続先、phase、outcome |
| `tasks`      | Run ID、Task ID、実行定義ハッシュ、依存条件                   |
| `attempts`   | Attempt ID、Runner情報、PIDと起動証跡、readiness、終了結果    |
| `resources`  | 種別、外部ID、所有権、作成操作、削除状態                      |
| `operations` | 受付、開始、外部要求、結果確定、状態不明                      |
| `events`     | 発生元、Run、順序、受信時刻、正規化したイベント               |
| `approvals`  | repo、設定ハッシュ、許可内容、承認時刻、有効範囲              |

秘密値をDBへ保存しない。診断情報にも環境変数全体やコピーしたファイル内容を含めない。

TaskRunnerはDBへ直接書かず、Attempt用の結果ファイルを一時ファイルからの置換で確定する。Coordinatorが取り込む。結果取り込みを繰り返しても同じAttemptの結果を重複適用しない。

スキーマ版を持ち、更新前にバックアップを作成する。旧版が読めないDBへ戻す場合はエラーを返し、未知の列や状態を黙って捨てない。

## 関連文書

[状態モデル](state-model.md) / [復旧](recovery.md) / [秘密情報](security.md)
