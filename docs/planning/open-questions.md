---
id: PLAN-QUESTIONS
title: '未決事項'
status: draft
documentVersion: '0.2'
updated: '2026-11-05'
---

# 未決事項

[文書目次](../README.md) · [更新ルール](../maintenance.md)

> statusは`draft`です。実装済みとして扱う範囲は、コードと試験結果で確認します。

この文書の管理対象：未確定の契約、互換性、実装前に決める項目。

<a id="source-25"></a>

## 設計上の保留事項

| 論点                                                  | 現時点の方針                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| ----------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| <a id="q01"></a>Q01：公開名とplugin ID                | 2026-11-06に調査した。`herdr-workflow`はPyPIで別の利用者の同名パッケージ(Herdr向けのmulti-agent workflow)が公開済みで、GitHubにも同名・類似名のHerdr関連リポジトリが多数ある(crates.ioとnpmは未使用)。公開名は、crates.io・npm・PyPI・GitHubのいずれでも未使用だった`herdr-taskflow`とする。改名(crate名、実行ファイル名、plugin ID、環境変数`HERDR_WORKFLOW_*`、リポジトリ名)は、リポジトリ名の変更とあわせて公開前に一括で行い、それまでは`herdr-workflow`を作業名として使う |
| <a id="q02"></a>Q02：Herdr対応下限                    | 必要APIの実機確認後に決定                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| <a id="q03"></a>Q03：任意Task一覧のサイドバー表示     | 解決済み。詳細なTask一覧は端末画面で表示し、サイドバーにはworkspaceのtoken(`flow_state`・`flow_running`・`flow_failed`・`flow_progress`)で集計値だけを出す。v0.9.5の公開APIに任意の区画を追加する手段はなく、ネイティブ区画はHerdr本体への変更案として扱う([Herdr連携](../integrations/herdr.md#sidebar-tokens))                                                                                                                                                               |
| <a id="q04"></a>Q04：対話RunnerのPTYと終了管理        | 解決済み。`runner: pty`としてTaskRunnerが擬似端末を持ち、`term`で接続する([ADR-0008](../decisions/0008-pty-runner.md))。キー入力、Ctrl+C、端末サイズ、切り離しをLinuxで試験した([T40](../testing/acceptance.md#t40)〜[T43](../testing/acceptance.md#t43))。macOSとHerdr実機のpaneでの確認は残る                                                                                                                                                                                |
| <a id="q05"></a>Q05：Windowsネイティブ                | named pipeとJob Object等の実装後に対応判定                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| <a id="q06"></a>Q06：複数worktreeをまたぐ親DAG        | 一つのRunと一つのworktreeの仕様が安定した後に追加                                                                                                                                                                                                                                                                                                                                                                                                                              |
| <a id="q07"></a>Q07：複数の実行結果をまたぐキャッシュ | 初期版では提供しない                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| <a id="q08"></a>Q08：branch別profileと外部プラグイン  | 解決済み。branch別の`profiles`(入力の既定値と`bootstrap.targets`の置き換え)と、Herdrプラグインのactionを実行記録で完了確認する`herdrAction`、同じworktreeイベントに反応する他プラグインの警告を実装した([外部連携](../integrations/external-tools.md))。actionの取消は公開APIに手段がなく、提供しない                                                                                                                                                                          |
| <a id="q09"></a>Q09：自動承認の詳細粒度               | 解決済み。許可項目ごとの承認は設けず、リポジトリ(共通Gitディレクトリ)・設定ハッシュ・Gitのhookとフィルターの要約が一致する一つの承認で設定全体を許可する([承認](../specs/security.md#approval))                                                                                                                                                                                                                                                                                |
| <a id="q10"></a>Q10：設定上限と性能値                 | grid各軸64、tabごとのpane数32、workspace全体のpane数128を既定値兼安全上限とし、設定では引き下げのみ許可する([設定](../reference/configuration.md))。初期版では上限を引き上げない。Herdr実機での応答時間の測定と、それに基づく引き上げの判断は残る                                                                                                                                                                                                                              |

## 追加の管理項目

以下は既存仕様で未定義の項目であり、新しい実装仕様の承認ではない。

| ID                  | 論点                     | 決める内容                                                                                                                                                                                                                                                                                                        | 対象文書                                                            |
| ------------------- | ------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------- |
| <a id="q11"></a>Q11 | 設定入力形式の未定義部分 | 入力形式は決定済み([設定](../reference/configuration.md#q11-keys))。設定型・意味検証・生成スキーマ・`secrets.yaml`の読み込みに加え、Run開始時の参照確認、Attempt起動直前の解決、ログのマスクまで実装した                                                                                                          | [設定](../reference/configuration.md)                               |
| <a id="q12"></a>Q12 | 保存先とIPCの契約        | LinuxとmacOSについて解決済み([ADR-0007](../decisions/0007-state-directory-ipc.md))。Windowsネイティブのパス、named pipe、プロセス識別は段階Eで決める                                                                                                                                                              | [状態ストア](../specs/persistence.md)・[復旧](../specs/recovery.md) |
| <a id="q13"></a>Q13 | 承認の成立条件           | 解決済み。設定ハッシュとGitのhook・フィルターの要約を承認に結びつけ、秘密値は名前だけを設定ハッシュで扱い、イベント経路の入力は既定値と`branch`に限る(`needs-input`で保留)。実行計画ハッシュとcheckout内容は承認に含めない([承認](../specs/security.md#approval))                                                 | [承認](../specs/security.md)                                        |
| <a id="q14"></a>Q14 | 拡張機能の受け入れ試験   | 対話RunnerのT40〜T43、profileのT44、外部アダプターのT45・T46を追加した。Windowsネイティブの試験IDは段階Eの保留とあわせて残る                                                                                                                                                                                      | [試験](../testing/acceptance.md)                                    |
| <a id="q15"></a>Q15 | 自動化と利用者操作の競合 | 解決済み。Run開始時・再開時のcheckoutの世代、HEAD、lockfile等の内容ハッシュと比べ、確認が必要な変更は一覧を示して端末の`y`か`--yes`で続ける([再開前の変更検出](../specs/recovery.md#resume-changes))。削除との競合は`cleanup plan`の表示と、保留があれば削除しない`remove`で扱う([後片付け](../specs/cleanup.md)) | [復旧](../specs/recovery.md)・[後片付け](../specs/cleanup.md)       |

## 決定時の処理

論点IDは削除せず、解決した内容、関連ADR、仕様変更、試験を追記する。未決のキーや既定値を実装で先に決めた場合も、レビュー前にその決定を記録する。

## 関連文書

[ADR](../decisions/README.md) / [設定](../reference/configuration.md) / [更新ルール](../maintenance.md)
