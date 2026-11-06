---
id: INT-HERDR
title: 'Herdr連携と表示'
status: draft
documentVersion: '0.2'
updated: '2026-09-16'
---

# Herdr連携と表示

[文書目次](../README.md) · [更新ルール](../maintenance.md)

> statusは`draft`です。実装済みとして扱う範囲は、コードと試験結果で確認します。

この文書の管理対象：API境界、接続先、表示方式、互換性試験。

<a id="source-14"></a>

## Herdr連携と表示

### 通信アダプター

HerdrAdapterにCLIとSocket APIの違いを閉じ込める。APIごとの要求・応答型を境界で変換し、内部のRunやTaskへHerdrのJSONを直接流し込まない。

CLIを呼ぶ場合は、Herdrが渡した`HERDR_BIN_PATH`を優先する。直接通信ではUnix domain socketとWindows named pipeを分離する。イベント購読用と通常の要求用の接続も分ける。[S1](../reference/sources.md#s1)[S2](../reference/sources.md#s2)

読み取り系の再試行と変更系の再試行を区別する。workspace作成等の応答を失った場合、同じ要求をそのまま再送せず既存リソースを照合する。接続先のサーバー世代が変わったときは、保存したpane IDをそのまま信用しない。世代を識別する情報が対象APIから得られない場合は、再接続のたびにリソースを再照合する。

### サイドバー

詳細なTask一覧は、プラグインの端末画面を初期の表示先とする。

Herdr側への補助表示には、公開メタデータAPIでworkspace単位の集計値を報告する案を採る。例は`flow_state`、`flow_failed`、`flow_running`である。表示に必要なHerdr設定の変更は案を提示し、利用者の設定を無断で書き換えない。

通常コマンドをAgentと偽って登録しない。公開APIで任意のTask一覧を独立したサイドバーセクションとして追加できることは確認できていないため、その機能は本体拡張の検討事項として残す。既存のAgent表示条件を操作するAPIと、任意のUI追加を区別する。[S1](../reference/sources.md#s1)

### 状態画面

状態画面にはRun、worktree、Task、Attempt、readiness、経過時間、終了理由、ログへの移動を表示する。状態だけでなく、待っている依存先と満たしていない条件も示す。

```text
Run: 01...  branch: feature/example  outcome: pending

TASK          PHASE       READINESS  DETAIL
toolchain     succeeded   -          exit 0
dependencies  succeeded   -          exit 0
generate      succeeded   -          exit 0
server        running     ready      127.0.0.1:3000
tests         running     -          attempt 1
developer     running     ready      observed: idle
```

停止、再試行、ログ表示、該当ペインへの移動を提供する。破壊的操作は状態画面からも確認を経て実行する。接続先が応答しない場合は、最終確認時刻を表示して古い状態であることを区別する。

<a id="implemented"></a>

### 実装済みの連携

`herdr-plugin.toml`に`[[events]]`で`worktree.created`を登録し、`herdr-workflow hook`が`HERDR_PLUGIN_EVENT`と`HERDR_PLUGIN_EVENT_JSON`を読む。イベントJSONからは`WorktreeInfo`(`path`、`branch`、`is_detached`、`is_linked_worktree`)だけを使い、外側の包み方には依存しない。この形式はHerdr v0.9.5タグの公開文書と`herdr-api.schema.json`、v0.9.0タグの文書で確認したものであり、実機でのイベント配送は試験していない。

`up --herdr`(または`attach --herdr`)では、要求元の`HERDR_SOCKET_PATH`へ1行1要求のJSONで次のメソッドを送る(`src/herdr.rs`)。

1. `worktree.mode: create`なら`worktree.create`(`cwd`、`branch`、`base`、`label`、`focus: false`)。応答の`workspace.workspace_id`と`worktree.path`を使い、workspaceを別途作らない。送る前に`pending:<repo>:<branch>`の受付記録を残し、このworktreeの`worktree.created`イベントでは別Runを開始しない(T12)。`attach`では要求元の`HERDR_WORKSPACE_ID`を使う。
2. 初期化対象のjobが成功した後、設定のtabごとに`layout.apply`(`workspace_id`、`tab_label`、`focus: false`、`root`)を送る。`tab_id`は渡さず、既存のtabを置換しない。分割木の比率は`firstSpan / totalSpan`とする。ログpaneは`herdr-workflow logs RUN --task TASK --follow --keep`、状態paneは`herdr-workflow status RUN --watch --keep`を`command`として起動し(Runの終了後も終えず、`resume`・`retry`の新しいAttemptを表示し続ける)、Shell・Agentのpaneはコマンドなしのshellとする。応答の`layout.root`の葉の順序で設定のpane IDとHerdrのpane IDを対応づける。
3. Agent paneでは`agent.start`(`name`、`kind`、`pane_id`)を送る。プロンプトは送らない。Agentの依存先は初期化対象に限る。
4. `workspace.report_metadata`で`flow_state`を報告する(失敗しても実行は続ける)。
5. 起動したAgentごとに`agent.get`(`target`はAgent名)を一定間隔で送り、応答の`agent.agent_status`(`idle`・`working`・`blocked`・`done`・`unknown`)を観測値として記録する。エラーコードに`not_found`を含む応答はAgentの終了または置換とみなして追跡を終え、それ以外の失敗は観測値を`unknown`にする。照会間隔は既定2秒で、診断のために要求元の環境変数`HERDR_WORKFLOW_AGENT_POLL_MS`で変えられる。

応答を失った変更要求は再送しない。これらの形式はHerdr v0.9.5タグの公開文書とAPIスキーマで確認したものであり、偽のHerdrソケットを使う試験(`src/herdr.rs`・`src/coordinator.rs`)で要求形を確認している。配置したpaneとAgentの対応は状態ストア(`runs.herdr_state`)へ記録し、`resume`・`retry`では`pane.get`と`agent.get`で照合してから使う([復旧](../specs/recovery.md#agent-resume))。Coordinatorを再起動すると、Agentタスクは追跡が切れたものとして`stopped`になり、`resume`で照合できれば追跡を再開する。Herdr実機での動作は未試験である。

## 互換性の記録

対応下限は未決定。対象Herdr版、APIスキーマの取得結果、CLI/TUI双方のイベント発火、OS、試験日、結果を実装時に記録する。取得できない項目は未確認のまま残す。

2026-09-14、インストール済みのHerdr v0.9.0(macOS)で`herdr api schema --json`を実行し、`herdr-plugin.toml`のマニフェスト形式(必須フィールド`id`/`name`/`version`/`min_herdr_version`、`[[actions]]`/`[[panes]]`/`[[startup]]`/`[[events]]`/`[[link_handlers]]`/`[[build]]`の各セクション)が存在することを確認した。フィールド構成の詳細は[CLIとHerdr action](../reference/cli.md)を参照する。この確認は対応下限の決定ではなく、マニフェスト形式が実在し公開ドキュメント([S1](../reference/sources.md#s1)[S2](../reference/sources.md#s2))と一致することの確認に限る。

2026-09-16、稼働中のHerdr v0.9.0(Linux、client/serverともprotocol 22)について、既定設定、pane CLI、Socket APIスキーマを確認した。pane数を制限する公開設定、APIフィールド、配列の`maxItems`、上限超過エラーは確認できなかった。この結果はHerdr内部に実装上の限界が存在しないことを保証しないため、Herdr Workflow側でtabごとのpane数を32以下に制限し、上限変更時は対象Herdr版で実動作を測定する。

任意Task一覧をネイティブサイドバーへ追加する機能は提供可能と断定しない。状態画面と集計メタデータの経路を先に検証する。

## 関連文書

[外部仕様と資料](../reference/sources.md) / [配置](../specs/layout.md) / [再接続時の照合](../specs/recovery.md)
