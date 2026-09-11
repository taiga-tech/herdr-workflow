# Herdr Workflow 設計書

| 項目                   | 内容                                                  |
| ---------------------- | ----------------------------------------------------- |
| 文書版                 | 0.1                                                   |
| 作成日・公開資料確認日 | 2026-09-11                                            |
| 状態                   | 実装前の設計案                                        |
| プラグイン名           | `herdr-workflow`（仮称。名称の使用可否は未調査）      |
| 実装言語               | Rust（ユーザー決定）                                  |
| 設定形式               | YAML                                                  |
| 配布形態               | Herdrプラグインと単独CLIを同じバイナリで提供する案    |
| 初期の対応対象         | Linux、macOS、WSL内のLinux版Herdr                     |
| 後続の対応対象         | Windowsネイティブ。通信とプロセス制御を個別に検証する |

## 文書の読み方

Rustの採用と、worktree作成から初期化、明示的DAG、ペイン配置、コマンド／Agent管理まで統合する方針は、これまでの会話を引き継いでいる。製品名、設定キー、CLI名、既定値、開発段階の区分は今回の提案である。

本文の「設計」はこれから実装する仕様を表す。Herdrの提供済み機能は「確認した外部仕様」と参考資料で区別する。設定例とコマンド例は設計案であり、現在のHerdrへそのまま渡して動作するものではない。実機でのHerdr連携試験、Rustのビルド、エージェント起動試験は実施していない。

---

## 1. 目的

一つの設定ファイルから、Git worktreeの準備、ローカル設定のコピー、開発環境の初期化、タスクの依存関係、Herdrの作業画面、実行状態を管理する。

対象の操作は次の流れとする。

```text
設定を検証し、実行内容を承認する
  → worktreeを作成、または明示的に既存worktreeを採用する
  → .env等を指定に従ってコピーする
  → mise / pnpm / コード生成等を依存順に実行する
  → Herdrのtab / paneを配置する
  → サーバー、テスト、Agentを依存条件に従って起動する
  → 状態とログを確認する
  → 停止、再試行、後片付けを行う
```

Git hookや複数プラグインの発火順には依存せず、本プラグインの実行管理部が順序を決める。既存プラグインを利用する場合も、同じ実行管理部を通して呼び出す。

Git worktreeを分けても、ポート、データベース、外部API、ユーザーのホームディレクトリまで分離する設計にはならない。これらを共有するタスクには個別の設定や排他制御を設ける。コンテナによる隔離は初期版の範囲外とする。

## 2. 確認した外部仕様

| 確認事項                                                                                                               | 設計への反映                                                             |
| ---------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------ |
| Herdrの`worktree.create`はcheckoutとHerdr workspaceをまとめて作成する。[S1]                                            | 作成後にworkspaceをもう一度作らない。返却されたIDを使う。                |
| `layout.apply`は分割木からタブを作成する。既存タブを指定すると置換され、実行中プロセス等は維持されない。[S1]           | 通常の再開処理で既存タブを置換しない。                                   |
| 公開APIにはイベント購読、Agent操作、メタデータ報告、プラグイン用端末画面がある。[S1][S2]                               | 状態取得と表示に使う。任意のサイドバー部品追加は前提にしない。           |
| Herdrのstartup hookは一回実行する初期化処理であり、daemonの監視機構ではない。[S2]                                      | hookは受付と起動確認だけで終了する。実行管理部の復旧はプラグインが扱う。 |
| `worktree-bootstrap`の作者は、作成イベント処理と別hookとの順序保証がないと説明している。[S3]                           | 初期化完了をDAGの依存条件にする。                                        |
| `workspace-manager`は宣言的配置とAgent起動を実装しているが、その後のペイン内プロセスのライフサイクルは管理しない。[S4] | 配置機能と継続的なプロセス管理を分離して実装する。                       |
| `serde_yaml`は保守終了を明記している。[S5]                                                                             | 前回の候補を変更し、YAML実装には`serde-saphyr`を採用候補とする。[S6]     |
| Tokioの`Child`は、破棄しただけでは既定で子プロセスを停止しない。[S7]                                                   | 停止要求、猶予期間、強制終了、終了回収を明示的に実装する。               |

Herdrの対応下限はまだ決めない。使用するAPIの存在と返却形式を検証し、その結果を`min_herdr_version`へ反映する。インストール済みバイナリのAPIスキーマ取得機能を互換性試験に利用する。[S1]

## 3. 機能範囲

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

## 4. 実行単位と所有権

| 用語           | 定義                                                                   |
| -------------- | ---------------------------------------------------------------------- |
| Repository     | 同じ共通Gitディレクトリを参照するリポジトリ                            |
| Checkout       | 実際の作業ディレクトリ。main worktreeとlinked worktreeを区別する       |
| Run            | 承認済み設定を、一つのcheckoutへ適用する実行単位                       |
| Task           | DAG上の処理。commandまたはagent                                        |
| Attempt        | Taskの一回の起動試行。再試行のたびに別IDを発行する                     |
| Resource       | プラグインが作成または明示的に採用したworkspace、tab、pane、プロセス等 |
| SessionContext | 接続先Herdrと、そのサーバー世代を識別する情報                          |

初期版は一つのRunに一つのworktreeを対応させる。Agentごとに編集先を分ける場合は、異なるworktreeを持つRunを作る。同じworktreeで複数Agentを起動しただけでは、編集ファイルは分離されない。

リソースには`owned`と`borrowed`を記録する。Runが作成したリソースのみを自動停止・削除の対象とし、既存worktreeや利用者のペインは既定で`borrowed`とする。パスが同じという理由だけでは所有権を推定しない。

## 5. 全体構成

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

### 5.1 Coordinator

同一ユーザーのプラグイン状態ディレクトリに対して一つ起動する。複数HerdrセッションのRunを扱うが、接続先IDを全操作に付ける。現在フォーカスされているペインへ暗黙に命令を送らない。

Coordinatorは受付キュー、実行計画、依存判定、所有権、停止要求を管理する。起動時にOSの排他ロックを取得する。単なるPIDファイルの有無では多重起動を判定しない。

### 5.2 TaskRunner

コマンドごとに独立したTaskRunnerを起動し、そのTaskRunnerが実際の子プロセスを監視する。Coordinatorの再起動と子プロセスの終了判定を分けるためである。

TaskRunnerはAttempt ID、起動証跡、終了結果、ログ位置を保存する。Coordinatorが停止しても、既に動いているプロセスのログと終了結果を保存できる構成にする。ただしTaskRunner自身の異常終了時まで、プロセスの状態を確定できるとは扱わない。

### 5.3 Herdr hook

hookは環境変数からイベントと対象を受け取り、Coordinatorの起動を確認して実行要求を送る。受付が永続化された後に終了する。

長い`pnpm install`や常駐サーバーをhook本体に置かない。プラグインをlinkした直後にも動くよう、各actionとevent受付がCoordinatorを起動確認する。startup hookの発火だけに依存しない。

## 6. 設定例

保存先の案は`.herdr/workflow.yaml`とする。以下はNext.jsとPrismaを使うプロジェクトの例である。`mise.toml`、Prisma schema、`test:e2e`スクリプトが対象プロジェクトに存在する前提であり、プラグインが自動検出して追加するものではない。

```yaml
# herdr-workflow が実装後に読み込む設定案。Herdr 本体の設定形式ではありません。
version: 1
name: web-development

inputs:
    branch:
        type: string
        required: true
    port:
        type: integer
        default: 3000

worktree:
    mode: create
    branch: '${inputs.branch}'
    base: main
    existingBranch: use
    ifAlreadyCheckedOut: error

files:
    source: primary
    copy:
        - from: .env.local
          to: .env.local
          optional: true
          ifExists: error
    symlinks: reject
    requireIgnored: true

defaults:
    cwd: '${worktree.path}'
    env:
        NODE_ENV: development
    jobTimeoutSeconds: 900

bootstrap:
    targets: [generate]

tasks:
    toolchain:
        type: command
        lifecycle: job
        runner: supervised
        argv: [mise, install]
        timeoutSeconds: 600

    dependencies:
        type: command
        lifecycle: job
        runner: supervised
        argv: [mise, exec, --, pnpm, install, --frozen-lockfile]
        dependsOn:
            - task: toolchain
              condition: succeeded

    generate:
        type: command
        lifecycle: job
        runner: supervised
        argv: [mise, exec, --, pnpm, exec, prisma, generate]
        dependsOn:
            - task: dependencies
              condition: succeeded

    server:
        type: command
        lifecycle: service
        runner: supervised
        argv:
            - mise
            - exec
            - --
            - pnpm
            - exec
            - next
            - dev
            - --hostname
            - '127.0.0.1'
            - --port
            - '${inputs.port}'
        env:
            PORT: '${inputs.port}'
        dependsOn:
            - task: generate
              condition: succeeded
        waitFor:
            type: http
            url: 'http://127.0.0.1:${inputs.port}/'
            status: 200
            timeoutSeconds: 120
            intervalMilliseconds: 500
            consecutiveSuccesses: 2
        stop:
            graceSeconds: 10

    tests:
        type: command
        lifecycle: job
        runner: supervised
        argv: [mise, exec, --, pnpm, run, 'test:e2e']
        env:
            BASE_URL: 'http://127.0.0.1:${inputs.port}'
        dependsOn:
            - task: server
              condition: ready
        onDependencyLost: stop
        timeoutSeconds: 600

    developer:
        type: agent
        agent:
            kind: claude
            # 起動だけを行い、プロンプトは自動送信しない。
        dependsOn:
            - task: generate
              condition: succeeded

workspace:
    label: '${inputs.branch}'
    initialTab: preserve
    tabs:
        - id: development
          label: development
          layout:
              type: grid
              columns: 6
              rows: 2
          panes:
              - id: A
                label: server
                view: { type: logs, task: server }
                placement: { column: 1, row: 1, colSpan: 3, rowSpan: 1 }
              - id: B
                label: developer
                view: { type: agent, task: developer }
                placement: { column: 4, row: 1, colSpan: 3, rowSpan: 1 }
              - id: C
                label: tests
                view: { type: logs, task: tests }
                placement: { column: 1, row: 2, colSpan: 1, rowSpan: 1 }
              - id: D
                label: workflow
                view: { type: status }
                placement: { column: 2, row: 2, colSpan: 4, rowSpan: 1 }
              - id: E
                label: shell
                view: { type: shell }
                placement: { column: 6, row: 2, colSpan: 1, rowSpan: 1 }

execution:
    maxConcurrentJobs: 4
    maxLiveServices: 8
    maxLiveAgents: 4
    onFailure: stop-dependents
    retries: 0

cleanup:
    onStop:
        processes: stop-owned
        panes: preserve
        worktree: preserve
        branch: preserve
```

`mise exec -- ...`を経由し、対象worktreeのmise設定で後続コマンドを実行する。`mise install`後に、親プロセスのPATHが自動更新されたと仮定しない。[S8]

この例の`PORT`は実行ごとに変更できる。並行する別Runへ同じポートを指定した場合は、利用中であることを検出して起動を止める。空きポートの検査と実際のbindの間には競合余地があるため、ポート番号の確保を検査だけで保証しない。

### 6.1 設定の規則

設定キーはcamelCaseとし、未知のキーと重複キーをエラーにする。`version`は設定スキーマの版であり、プラグインのリリース番号とは分ける。

Task IDとpane IDは明示する。表示名は識別子として使わない。Agentタスクは一つのAgent用ペインへ対応づける。ログ表示は一つのタスクに複数置いても、プロセスを再起動しない。

コマンドは`argv`を標準形式とする。引数に空白が含まれていても一つの要素として扱う。シェル構文が必要な場合だけ、実行シェルとscriptを別項目で明示する。`argv`とscriptの同時指定は拒否する。

`${inputs.port}`等はプラグインが定義した値置換であり、シェル評価ではない。未定義の変数、型不一致、パス制約違反は実行前にエラーにする。引数配列は置換後も分割しない。

YAMLの外部ファイル読み込み、任意のカスタムタグ、暗黙の実行式は初期版で扱わない。ファイルサイズ、深さ、alias展開量に上限を設定する。

### 6.2 読み込み元と承認

明示した`--config`を優先し、それ以外は作成元の`.herdr/workflow.yaml`を使う。ユーザー固有の承認と上書き設定は`HERDR_PLUGIN_CONFIG_DIR`側で管理し、承認情報をリポジトリへコミットしない。

Run開始時に設定を解決し、ハッシュ付きの実行計画として固定する。worktree作成後に別ブランチの同名設定へ自動的に読み替えない。

## 7. DAGとスケジューラ

### 7.1 依存条件

| 条件        | 成立する状態                                                             | 主な用途                                           |
| ----------- | ------------------------------------------------------------------------ | -------------------------------------------------- |
| `succeeded` | commandのjobが終了コード0で終了                                          | install後のコード生成                              |
| `ready`     | serviceが生存し、readinessを満たす。AgentではHerdrの起動準備確認を満たす | サーバー起動後のE2E                                |
| `started`   | Attemptのプロセス起動を確認済み                                          | 起動だけで足りる補助処理。設定で明示した場合に限る |

依存条件の省略は認めない。serviceへ`succeeded`を指定する等、通常は満たせない指定を検証時に拒否する。Agentの`idle`や`done`を、プログラムの終了コード0と同じ意味では使わない。

依存先が複数ある場合はAND条件とする。OR条件と動的なタスク追加は初期版で扱わない。

### 7.2 初期化と画面構築の順序

`bootstrap.targets`に指定したjobと、その依存先を初期化対象とする。初期化の中にserviceやAgentを含めない。

内部の実行計画には、worktree準備、ファイルコピー、レイアウト作成も名前付きの操作ノードとして含める。利用者は`plan --json`で、設定から補われた依存関係も確認できる。

```text
@worktree.ensure
       |
@files.copy
       |
toolchain -> dependencies -> generate
                                  |
                            @layout.create
                              /          \
                           server      developer
                             |
                       HTTP ready確認
                             |
                            tests
```

初期化対象ではないタスクは、初期化成功と画面構築の完了後に開始する。単独CLIのheadless実行では画面ノードを省き、Agentとpane依存タスクを含む計画は拒否する。

### 7.3 並行実行

jobの同時実行数、常駐service数、Agent数は別々に制限する。常駐serviceがjob用の枠を保持し続け、後続のテストが起動できなくなる状態を避ける。

実行可能タスクの選択はTask ID順を既定とし、同じ計画で起動順の説明が変わらないようにする。開始前にはcheckout単位と利用リソース単位のロックも確認する。

プラグインが自動投入する、同じcheckoutへ書き込むAgentの作業やコード生成処理の並行実行は、設定者が共有書き込みを許可した場合に限る。利用者の手動操作まで排他制御できるとは扱わない。複数の独立した実装作業には別Runと別worktreeを使う。

### 7.4 失敗とキャンセル

初期化が失敗した場合は、レイアウトの追加と後続タスクの起動を行わない。worktreeと失敗ログは残す。

通常タスクの既定動作は`stop-dependents`とする。未起動の依存タスクを`skipped`にし、実行中の依存タスクには`onDependencyLost`を適用する。独立したタスクの扱いは実行計画に明示する。

停止処理は依存の逆順で行う。テストを止めた後にサーバーを止める等、利用側を先に停止する。自動再試行は既定で0回とし、明示設定したタスクだけを再試行する。

## 8. gridとペイン配置

### 8.1 座標と範囲

`column`と`row`は1始まり、`colSpan`と`rowSpan`は正の整数とする。列幅と行高は等分を初期仕様とし、比率指定は後続の拡張に分ける。

前節の配置は次のセル割り当てになる。

```text
A A A B B B
C D D D D E
```

```text
+-----------+-----------+
|     A     |     B     |
+---+-------+-------+---+
| C |       D       | E |
+---+---------------+---+
```

検証では範囲外、重なり、未配置セル、参照切れを拒否する。空き領域を残す場合は、明示的にshell用のペインを配置する。

### 8.2 分割木への変換

Herdr側のレイアウトは二分割木で表現されるため、gridを内部の分割木へ変換する。[S1]

変換器は、ペインを横切らずに領域を二分できる行境界または列境界を探し、両側へ再帰する。候補の探索順を固定する。左右分割は`right`、上下分割は`down`に変換する。

任意の矩形配置を二分割木へ変換できるとは扱わない。分割できない配置は`layout_not_sliceable`として事前に拒否し、見た目を近似した配置へ黙って変更しない。

端末の列数・行数と罫線幅を踏まえて実際の大きさを計算する。端数処理を固定し、各ペインの最低寸法を下回る場合は配置を開始しない。縮小された端末への対応では、実行中タスクを作り直さず表示上の制約として扱う。

### 8.3 再適用

作成直後は新しいタブを追加する。Herdrがworktree作成時に用意した最初のタブは、既定で残す。

`resume`は既存タブへ`layout.apply`を再実行しない。所有するペインIDと現在の構成を照合し、欠損や利用者の変更を検出した場合は差分を提示する。配置だけを変える処理と、タブを置換する処理は別コマンドにする。

## 9. 状態モデル

### 9.1 Taskの実行状態

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

### 9.2 Runの状態

Runも進行状態と結果を分ける。

```text
phase   = queued / preparing / active / stopping / stopped / finished
outcome = pending / succeeded / failed / cancelled / unknown
```

例えばE2Eが失敗しても、独立したサーバーやAgentを残した場合は`phase=active, outcome=failed`になる。状態画面には稼働中リソース数も表示する。

常駐serviceを含むRunは、準備完了しただけでは終了成功にならない。`ready`は待ち合わせ用の条件として扱い、`finished`と分ける。

### 9.3 起動世代

再試行ごとにAttempt IDを変える。古いAttemptのログ、readiness、終了通知によって新しいAttemptを更新しない。

serviceの起動直後にプロセスが終了した場合は、HTTP応答が返っていても`ready`にしない。プロセスの生存、対象Attempt、readiness確認結果を同じ判定時点で照合する。

## 10. コマンドとAgentの実行

### 10.1 非対話コマンド

初期版のcommandは`runner: supervised`を中心に実装する。TaskRunnerが子プロセスの標準出力と標準エラーを取得し、終了コードを記録する。Herdrのペインにはそのログを表示する。

ログ画面を閉じてもプロセスは停止しない。プロセスの停止は`stop`やTaskの取消操作で行う。ログ用ペインを再表示しても同じTaskへ接続するだけで、コマンドを起動し直さない。

jobの標準入力は既定で閉じる。入力を要求して停止したコマンドに対して、任意の応答を自動入力しない。実行期限とログを通して利用者へ失敗を返す。

### 10.2 対話コマンド

対話入力が必要なコマンドには、後続段階で`runner: pane`を用意する。HerdrのPTY内で専用ラッパーを動かし、ラッパーから起動と終了の結果を報告する。

この方式では、端末へのキー入力、前景プロセスグループ、Ctrl+C、端末サイズ変更を維持する試験を行う。非対話版のパイプ接続へ置き換えて済ませない。

PTY上の出力は標準出力と標準エラーを区別できない場合がある。対話コマンドについて、非対話版と同じ二系統の完全なログを提供すると約束しない。通常のshellペインを置く機能と、対話コマンドの管理機能は別とする。

### 10.3 Agent

AgentはHerdrのAgent起動機能を使い、ペインへの文字列入力だけで起動成功を推定しない。認識されたAgentの識別情報と対象paneを記録する。

プロンプト送信は設定で明示した場合のみ行う。再接続時に同じプロンプトを自動送信し直さない。応答を失った場合は、受付と実行の状態を照合できるまで`unknown`として扱う。

Agentが待機状態になっただけでは開発作業の成功を判定しない。後続のテスト、生成物の検査、明示的な結果ファイル等を別jobとして定義する。

### 10.4 プロセス停止

Unixでは本プラグインが起動したプロセスグループを記録し、通常の停止信号、猶予期間、強制終了、子プロセスの終了回収の順に処理する。WindowsではJob Object等による子孫プロセス管理をOS別実装で検証する。

PIDだけで対象を決めず、起動時刻、所有Run、Attempt、コマンド情報も照合する。終了結果が不明なときに、同じ番号を再利用した別プロセスへ信号を送らない。

プロセスが自分で別セッションへ離脱する動作は初期版の管理対象外とする。サービスはforegroundで動かす。`kill_on_drop`だけで子孫プロセスの停止を完了したと扱わない。[S7]

### 10.5 実行環境

`cwd`を各タスクへ明示し、hookの作業ディレクトリや利用者の現在ディレクトリに依存しない。

環境変数は起動元の環境に共通設定、タスク設定を順に適用する。秘密値は別の参照形式で扱い、解決済みの値を計画JSONへ書き出さない。`HERDR_*`等のホスト管理用識別子を設定ファイルから上書きさせない。

同じcheckout内の初期化は一つに制限する。別checkoutの初期化は並行できるが、共有するDBや外部環境を変更するタスクには明示的なresource lockを付ける。ロックは本プラグイン経由の実行を調停するものであり、他アプリケーションまで制御するものではない。

## 11. readinessとログ

### 11.1 確認方式

| 方式  | 検査内容                                                | 制約                                       |
| ----- | ------------------------------------------------------- | ------------------------------------------ |
| HTTP  | URL、期待status、連続成功回数。必要に応じて本文やheader | 既存の別サーバーを誤認しない確認が必要     |
| TCP   | 指定host・portへ接続できること                          | アプリケーションの機能正常までは判定しない |
| log   | 現在のAttemptが出したログに指定パターンが出ること       | 過去ログを対象にしない                     |
| Agent | HerdrによるAgent起動・入力準備の確認                    | 作業内容の完了とは分ける                   |

HTTPとTCPは既定でloopbackを対象とする。外部URLは設定で明示する。HTTP確認では応答サイズ、接続期限、リダイレクト回数も制限する。

ポートの事前使用確認に加え、対応できるアプリケーションではRun IDを返すhealth endpointを利用する。汎用のHTTP 200だけではプロセスの同一性まで保証できないことを表示する。

readiness待ちの間にプロセスが終了したら、その時点で待機を終了する。一定時間sleepしただけで`ready`にしない。

### 11.2 準備後の状態低下

serviceの確認を継続し、設定した回数以上失敗したら`unready`へ変える。新しい依存タスクは起動しない。実行中の依存jobは既定で停止する。完了済みjobを後から失敗へ書き換えることはせず、実行履歴に依存先の状態低下を追加する。

serviceが予期せず終了した場合は、終了コード0でも`unexpected-exit`として扱う。利用者が停止要求を送っていた場合は`stopped`にする。

### 11.3 ログ形式

非対話コマンドのログは、`run_id`、`task_id`、`attempt_id`、時刻、stream種別、連番、データを持つ。stdoutとstderrそれぞれの順序を保持するが、二つのstream間の完全な発生順までは保証しない。

ファイルはAttemptごとに分ける。既定案は1ファイル20 MiB、5世代とし、保持期間と総容量を設定可能にする。上限到達時にも子プロセスの出力を読み取り続け、古いログをローテーションする。

制御文字を除いた表示用ログと保存形式を分ける。画面表示時は危険な端末制御シーケンスを無条件に再生しない。

秘密値のマスクは指定された値や規則を対象とする。コマンドが別形式で出力する秘密情報をすべて検出できるとは扱わない。ファイル内容や環境変数全体のデバッグ出力は行わない。

ログ書き込みに失敗したら`log-degraded`を記録して通知する。状態ストア自体への書き込みが失敗した場合は、新しいタスクの起動を止める。

## 12. worktreeとファイル操作

### 12.1 作成

Herdrを利用する通常経路では、HerdrAdapterからworktree作成を依頼し、応答に含まれるworktreeとworkspaceの対応を保存する。別途workspaceを追加作成しない。

既存ブランチの利用と新規ブランチ作成を区別する。`base`は新規ブランチを作る場合に使い、既存ブランチを指定した際にその内容を`base`へ巻き戻さない。

別worktreeでcheckout済みのブランチに対して強制実行しない。既存checkoutの採用は`attach`で明示する。main worktreeを自動イベントの初期化対象に含めない。

Gitの情報はCLIから取得し、`.git`ファイルや管理ディレクトリを独自に編集しない。作業ツリー一覧、共通Gitディレクトリ、対象ブランチを照合する。[S9]

### 12.2 ファイルコピー

`files.source: primary`は、Gitが認識するmain worktreeを指す。ブランチ名が`main`であるディレクトリを探すという意味ではない。bareリポジトリ等でコピー元を確定できない場合は、コピーを開始せず理由を返す。

コピーするパスは許可リスト方式とする。初期版の既定では、Git未追跡かつignore対象のファイルに限る。`.env`を含め、指定されていないファイルはコピーしない。`node_modules`等を自動で追加しない。

絶対パス、`..`による脱出、親子で重複する指定、コピー先の追跡済みファイルを拒否する。途中の親ディレクトリと対象ファイルのsymlinkも既定で拒否する。検査後の差し替えに備え、実際の操作時にもパスと所有対象を再確認する。

対象が既に存在する場合の既定動作はエラーとする。`skip`または`replace`は明示指定した場合だけ許可する。`replace`時は一時領域に準備してから置き換え、操作記録を残す。

一連の複数ファイル操作を、OS全体として不可分なトランザクションだとは扱わない。途中で失敗した場合は、本プラグインが記録した変更だけを逆順に復旧する。実行中に利用者が変更したファイルは無条件で巻き戻さない。

### 12.3 Git hookとの関係

本プラグインは`core.hooksPath`や既存hookを変更しない。既存hookが同じ初期化を行う場合は、二重実行になり得ることを検査結果へ表示する。

Gitの`post-checkout`はworktree作成以外でも実行される。`.git`がファイルかどうかだけでは「作成時に一度だけ」を判定できない。[S10] 本プラグインでは、イベント受付とRunの記録で初回適用を判定する。

Git側で有効なhookやフィルターもコマンド実行につながるため、worktree作成前の承認でその存在を確認対象に含める。Gitの通常動作を秘密裏に無効化しない。

## 13. イベント、排他制御、再開

### 13.1 イベント受付

中心となるイベントは`worktree.created / worktree.opened / worktree.removed`とする。作成イベントの配送経路は対象Herdr版のCLIとTUIの両方で試験する。

イベント経路では、通知に含まれる作成済みcheckoutとworkspaceを対象にする。設定例の`worktree.mode: create`を見て別worktreeを作り直さない。内部の`@worktree.ensure`は既存checkoutの照合へ置き換え、`${inputs.branch}`はイベントのブランチ情報から解決する。ブランチ名を確定できないdetached checkoutは、自動適用せず明示的な設定を求める。

自動初期化は、ユーザー設定で許可されたリポジトリにだけ適用する。新しいworktreeを発見しただけで、任意のリポジトリの設定を実行しない。`worktree.opened`では、既存の成功済み初期化を既定で繰り返さない。

必要なHerdr版に限り、`workspace.created`等からworktree情報を照合する互換処理を用意する。フォーカス切替を初期化の一般的なトリガーにはしない。

### 13.2 自分が起こしたイベント

プラグインの`up`がworktreeを作ると、自分宛てにも作成イベントが届く。このイベントを別Runとして起動しない。

作成要求前に、repo IDとブランチに対するpending operationを記録する。応答とイベントがどちらの順に届いても、その操作へ関連づける。関連づけられない場合は、GitとHerdrの実状態を照合するまで追加起動を保留する。

### 13.3 排他キー

少なくとも以下を区別する。

```text
作成ロック       : repo ID + branch
checkout実行ロック: repo ID + canonical checkout identity
自動受付の重複判定: checkout世代 + workflow ID + 設定ハッシュ
試行識別子       : run ID + task ID + attempt ID
```

異なるHerdrセッションでも、同じcheckoutの初期化は競合させない。削除後に同じパスへ作り直されたworktreeは新しい世代として扱う。

「イベントが一度だけ届く」ことも「副作用が一度だけ起きる」ことも前提にしない。受付を記録し、重複を抑え、不明な状態を照合する方式とする。

### 13.4 起動直後のクラッシュ

プロセス起動と起動記録の保存は、一つの不可分な操作ではない。起動後・応答保存前にクラッシュした場合は、コマンドが走った可能性を残す。

復旧時はRunnerの起動証跡、結果ファイル、OSのプロセス情報を照合する。結果を確認できない場合は`unknown`とし、同じコマンドを自動的に再起動しない。とくにDB更新、デプロイ、メール送信等を再実行しない。

### 13.5 再開と再試行

`resume`は保存済み計画を使って実状態を照合し、未実行であることを確認できたタスクから再開する。`retry`は新しいAttemptとして実行する。

初期版は、成功済みタスクを別Runへ横断して再利用するキャッシュを持たない。同じRun内で成功結果を使う場合も、設定、checkoutの同一性、出力や利用前提の変更を検査する。初期化後にlockfile等が変わった場合は成功済みという記録だけで省略しない。

## 14. Herdr連携と表示

### 14.1 通信アダプター

HerdrAdapterにCLIとSocket APIの違いを閉じ込める。APIごとの要求・応答型を境界で変換し、内部のRunやTaskへHerdrのJSONを直接流し込まない。

CLIを呼ぶ場合は、Herdrが渡した`HERDR_BIN_PATH`を優先する。直接通信ではUnix domain socketとWindows named pipeを分離する。イベント購読用と通常の要求用の接続も分ける。[S1][S2]

読み取り系の再試行と変更系の再試行を区別する。workspace作成等の応答を失った場合、同じ要求をそのまま再送せず既存リソースを照合する。接続先のサーバー世代が変わったときは、保存したpane IDをそのまま信用しない。世代を識別する情報が対象APIから得られない場合は、再接続のたびにリソースを再照合する。

### 14.2 サイドバー

詳細なTask一覧は、プラグインの端末画面を初期の表示先とする。

Herdr側への補助表示には、公開メタデータAPIでworkspace単位の集計値を報告する案を採る。例は`flow_state`、`flow_failed`、`flow_running`である。表示に必要なHerdr設定の変更は案を提示し、利用者の設定を無断で書き換えない。

通常コマンドをAgentと偽って登録しない。公開APIで任意のTask一覧を独立したサイドバーセクションとして追加できることは確認できていないため、その機能は本体拡張の検討事項として残す。既存のAgent表示条件を操作するAPIと、任意のUI追加を区別する。[S1]

### 14.3 状態画面

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

## 15. 状態ストア

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

## 16. 実行承認と秘密情報

Herdrプラグイン自体はユーザー権限で動くコードである。[S2] 本設計の承認機構もOSレベルの隔離ではない。

初回の自動実行は無効とする。`trust`で、リポジトリ、設定ハッシュ、実行コマンド、コピー対象、Agent起動、ネットワーク利用等を確認して許可する。設定変更時には再承認する。非対話のevent経路では、未承認の処理に`holdReason=needs-approval`を付けて保留する。

`mise`やパッケージ管理ツールの信頼確認を迂回するフラグを自動で付けない。miseにも独自の信頼管理があるため、本プラグインの承認と同じものとは扱わない。[S11]

`pnpm install --frozen-lockfile`でも、プロジェクトや依存パッケージ由来の処理を実行する可能性を前提に承認する。実行する対象コードへの信頼は残る。設定ファイルの承認だけで、checkout内の任意コードを安全と判定しない。

`.env`をworktreeへコピーすると、そのworktreeを読むAgentも秘密値を読める。コピー許可とAgentへのアクセス許可を同時に説明する。認証情報を渡したくない場合はコピー対象から外し、タスク単位の秘密値参照を使う。

Coordinatorの制御ソケット、状態ディレクトリ、結果ファイルは同じユーザーだけが利用できる権限にする。外部ネットワークへ制御APIを公開しない。送信者を検証しても、同一ユーザーの任意コードからの防御まで提供するとは扱わない。

## 17. 停止と後片付け

### 17.1 通常停止

通常の`stop`は、本プラグイン所有のタスクを依存の逆順に停止する。worktree、ブランチ、ログ、画面は既定で残す。停止後に利用者が結果を確認できる状態にする。

paneを閉じる操作とservice停止を分ける。管理対象Agentのペインに別プロセスが入っている等、所有状態が変わっていた場合は削除を保留する。

### 17.2 worktree削除

worktree削除は`remove`で明示する。削除前に計画を表示し、対象パス、ブランチ、所有Run、稼働プロセス、追跡ファイルの変更、未追跡ファイル、ignore対象ファイルを確認する。

`.env`等のignore対象ファイルも消えるため、通常の変更一覧に出ないことを理由に無変更と判断しない。確認後も削除直前に状態を読み直す。main worktree、借用したcheckout、識別が曖昧な対象は自動削除しない。

Herdr側のworktree削除はブランチを残す仕様なので、ブランチ削除は別の確認操作とする。[S1] リモート参照が消えたという理由だけで、ローカルブランチがmerge済みと判断しない。

### 17.3 外部からの削除

利用者がHerdr本体やGitからworktreeを削除した場合、プラグインが事前に阻止できるとは扱わない。削除イベントまたは照合で検出し、所有プロセスの停止と状態更新を試みる。

削除済みのパスへ新しいログやコピー先を作り直さない。リソースの削除と同名パスへの再作成を識別する。

### 17.4 ロールバック範囲

復旧対象は、本プラグインが作成したファイル配置とリソースに限る。`mise install`のユーザー共通環境への変更、DB migration、外部サービスへの操作、利用者やAgentの編集を自動で元に戻さない。

元に戻せない処理には、個別に補償用タスクを定義できる拡張を検討する。ただし補償タスクの実行も承認対象とし、処理失敗時に機械的にデータ削除へ進まない。

## 18. 技術構成

以下は依存候補と担当範囲である。実装着手時に利用版、MSRV、license、security advisory、ビルド対象の整合を確認し、`Cargo.lock`へ固定する。この文書では未確認の「最新版番号」を記載しない。

| 担当                          | 候補                                           |
| ----------------------------- | ---------------------------------------------- |
| 非同期処理、通信、プロセス    | `tokio`                                        |
| キャンセルと入出力補助        | `tokio-util`                                   |
| 設定型、JSON                  | `serde`、`serde_json`                          |
| YAML                          | `serde-saphyr` [S6]                            |
| CLI引数                       | `clap`                                         |
| 依存グラフ、循環検出          | `petgraph` [S12]                               |
| HTTP readiness                | `reqwest`                                      |
| ログパターン                  | `regex`                                        |
| 構造化ログ                    | `tracing`、`tracing-subscriber`                |
| 型付きエラー、CLI最上位エラー | `thiserror`、`anyhow`                          |
| 状態ストア                    | `rusqlite` [S13]                               |
| 端末画面                      | `ratatui`、`crossterm`                         |
| OS別のプロセス制御            | Unix用APIとWindows用APIを別moduleに隔離        |
| スキーマ出力                  | `schemars`等を検討。Rust型との二重管理を避ける |

非同期処理の中でSQLiteの同期処理を長時間実行しない。専用の書き込み処理へ要求を送り、トランザクションを短く保つ。

設定ファイルの常時監視は初期版に入れず、明示的な再読み込みを使う。実行中に設定変更を検出しても、そのRunの計画を書き換えない。

## 19. リポジトリ構成

単一crateから開始し、外部APIと実行管理をmoduleで分ける。

```text
herdr-workflow/
  Cargo.toml
  Cargo.lock
  mise.toml
  herdr-plugin.toml
  README.md
  docs/
    design.md
    configuration.md
    recovery.md
  schema/
    workflow.schema.json
  examples/
    workflow.yaml
  src/
    main.rs
    lib.rs
    cli.rs
    config/
      model.rs
      load.rs
      validate.rs
    plan/
      graph.rs
      compile.rs
      layout.rs
    runtime/
      coordinator.rs
      scheduler.rs
      runner.rs
      readiness.rs
      cancellation.rs
    herdr/
      client.rs
      transport.rs
      capabilities.rs
      events.rs
      types.rs
    worktree/
      inspect.rs
      create.rs
      adopt.rs
      remove.rs
    files/
      plan.rs
      copy.rs
      rollback.rs
    state/
      store.rs
      models.rs
      migrations.rs
      reconcile.rs
    security/
      approval.rs
      paths.rs
      redaction.rs
    platform/
      unix.rs
      windows.rs
    ui/
      status.rs
      logs.rs
  tests/
    config_cases/
    dag_cases/
    layout_cases/
    process_cases/
    integration/
    fixtures/
```

Rust型を設定定義の基準とし、構造スキーマはそこから生成する。参照整合性、依存の成立条件、gridの分割可能性等は意味検証として別途実装する。

`HerdrClient`、`ProcessBackend`、`StateStore`の境界を用意し、Herdrを起動しなくても計画器とスケジューラを試験できるようにする。内部エラーは種類と再試行可否を持ち、文字列の一致で分岐しない。

## 20. CLIとHerdr action

この節の`herdr-workflow`コマンドは、これから実装するCLI案である。

| コマンド                 | 操作                                                   |
| ------------------------ | ------------------------------------------------------ |
| `validate`               | 構文、型、参照、DAG、gridの検証。コマンドは実行しない  |
| `plan`                   | 作成、コピー、実行、配置を解決し、計画とハッシュを表示 |
| `trust`                  | 表示した設定と実行許可を承認する                       |
| `up`                     | 新規worktree用Runを受け付け、Run IDを返す              |
| `attach`                 | 明示した既存checkoutへ新しいRunを関連づける            |
| `run --target TASK`      | 対象タスクとその依存処理を実行し、対象の結果を待つ     |
| `wait RUN --until ready` | 初期化、配置、選択した常駐タスクの準備を待つ           |
| `status [RUN]`           | 実行状態、待機条件、失敗、最終確認時刻を表示           |
| `logs RUN --task TASK`   | 指定タスクのログを表示                                 |
| `stop RUN`               | 所有タスクを停止する。worktreeは残す                   |
| `resume RUN`             | 保存済み計画と実状態を照合して再開する                 |
| `retry RUN --task TASK`  | 指定タスクを新しいAttemptとして再試行する              |
| `cleanup plan RUN`       | 削除候補と保留理由を表示する                           |
| `remove RUN`             | 確認済みの所有worktreeを削除する                       |
| `doctor`                 | Herdr API、Git、mise、パス、通信、権限を検査する       |

```bash
# 構文と実行計画の確認
herdr-workflow validate --config .herdr/workflow.yaml
herdr-workflow plan --input branch=feature/example --input port=3001

# 承認とRunの作成
herdr-workflow trust --config .herdr/workflow.yaml
herdr-workflow up --input branch=feature/example --input port=3001

# upが返した実際のRun IDを指定する
herdr-workflow status RUN_ID
herdr-workflow logs RUN_ID --task server --follow
herdr-workflow stop RUN_ID
```

`up`の受付成功は、初期化の成功を意味しない。`up`は受付記録とRun IDを返し、成否は`wait`、`status`、通知で取得する。`run --target`はserviceの存在によって待ち続けず、指定jobの結果で終了する。

機械向け出力は`--json`で提供し、ログや進捗文を混在させない。終了コードの案は、0が操作成功、2が設定不正、3が承認待ち、4が実行失敗、5が接続・互換性エラー、6が状態不明または照合待ち、130が利用者の中止とする。生の子プロセス終了コードは結果オブジェクトにも保存する。

Herdr actionは`up / status / stop / validate / cleanup-review`を入口とし、実処理は同じCLIとCoordinatorへ集約する。選択式の操作は、manifestで宣言した端末画面を開いて入力を受け取る。manifestの`min_herdr_version`は互換性試験後に決める。

## 21. 既存プラグインと外部ツール

既存プラグインの再利用は許容する。ただしDAG、状態管理、実行順の制御を外部へ分散させない。

外部処理を一つのTaskとして扱う場合は、起動受付、処理完了、成功条件、キャンセル方法、ログの取得方法をアダプターで定義する。Herdrのaction呼び出しが返っただけでは、実際のセットアップ完了と判定しない。

| 対象機能                         | 本設計での扱い                                                  |
| -------------------------------- | --------------------------------------------------------------- |
| ファイルコピー                   | 原則として内部実装。コピー条件と復旧方法を同じ計画で扱う        |
| 初期化コマンド                   | 外部CLIをTaskRunnerから実行する                                 |
| 既存レイアウトプラグイン         | 手動呼び出しと完了確認が可能な場合に限定して接続                |
| 外部worktree管理ツール           | 作成結果のcheckoutとHerdr workspaceを照合するアダプターを設ける |
| fetch、upstream設定、pull/rebase | 明示設定した追加処理として扱う。既定では実行しない              |
| ブランチ名による構成選択         | 後続のprofile機能で、最初に一致したルールと選択結果をplanに表示 |
| 削除候補の検出                   | review用一覧を作る。リモート参照消失だけで自動削除しない        |

同じworktree作成イベントに反応する他プラグインが有効な場合は警告する。競合する初期化を止めるために、利用者のプラグインを自動無効化しない。

コードを取り込む場合はlicenseと著作権表示を調査する。公開APIだけで連携する方式と、コードを再利用する方式を分けて判断する。

## 22. 開発環境と配布

Rustのツールチェーンは`mise.toml`に具体的な版を固定し、CIも同じ版を利用する。依存関係は`Cargo.lock`を管理対象とする。実行対象プロジェクトのNode.jsやpnpmは、そのプロジェクトのmise設定に従う。

開発チェックのコマンド案は以下とする。

```bash
mise install
mise exec -- cargo fmt --all -- --check
mise exec -- cargo clippy --locked --all-targets --all-features -- -D warnings
mise exec -- cargo test --locked
mise exec -- cargo build --locked --release
```

配布ではOSとCPU別の成果物を作り、checksumと出所を検証できる情報を公開する。開発者向けにソースからのビルド経路も残す。

Herdrの`plugin install`によるbuild処理と、配布済みバイナリの利用を分けて案内する。Herdrは不足するビルド用ツールチェーンを導入しないため、ソースビルド経路ではRustの導入を前提とする。ローカルの`plugin link`前にもビルドを行う。[S2]

単一バイナリでも、外部の`git`、`herdr`、`mise`、対象コマンドは別途必要である。またOSの動的ライブラリへの依存はビルド条件によるため、配布物ごとに検査する。

Windowsネイティブは、Herdr側でもWindows向け機能の対応状況が個別に案内されている。[S14] named pipe、Job Object、パス表現、環境変数、終了信号、PTY連携の試験が通るまで、Unix版と同じ対応済み表示をしない。WSLはLinux版として扱い、Windows上のプロセスを同時に管理する機能は含めない。

プラグイン無効化時には新しいRunの自動受付と起動を停止する。既に稼働中のRunは無断で破棄せず、`stop`で停止する。Coordinatorは接続先ごとの有効状態を定期的に照合する。アンインストール前に稼働Runと残存状態を確認できる手順を用意する。

## 23. 実装段階

| 段階          | 実装対象                                               | 次へ進む条件                                  |
| ------------- | ------------------------------------------------------ | --------------------------------------------- |
| A：計画器     | 設定型、意味検証、DAG、grid変換、plan                  | 不正設定を外部変更前に拒否できる              |
| B：非対話実行 | Runner、job/service、readiness、停止、ログ、状態ストア | サーバー準備後にE2Eを開始し、失敗を再現できる |
| C：Herdr統合  | worktree、初期化、grid配置、Agent、状態画面            | 新規worktreeからの一連の処理が動く            |
| D：復旧と運用 | 重複抑止、クラッシュ照合、再開、承認、削除確認         | 二重起動と誤削除の試験が通る                  |
| E：拡張       | 対話Runner、外部ツール連携、profile、Windowsネイティブ | 各機能の契約試験とOS別試験が通る              |

段階AとBはHerdrなしでも試験する。Herdr統合に必要なAPIの調査は並行して行い、公開前にはCとDを含む受け入れ条件を満たす。

詳細なTask一覧のネイティブサイドバー追加は、公開APIだけで扱える範囲が確認できた後、またはHerdr本体への変更案として別途進める。状態画面の提供をその判断待ちにしない。

## 24. テストと受け入れ条件

| ID  | 試験内容                                        | 期待結果                                          |
| --- | ----------------------------------------------- | ------------------------------------------------- |
| T01 | 未知キー、重複キー、未定義変数                  | 外部操作前に位置付きエラー                        |
| T02 | 循環依存                                        | 循環するTask IDの経路を表示                       |
| T03 | serviceへの`succeeded`依存                      | 設定検証で拒否                                    |
| T04 | `mise install`失敗                              | pnpm、配置、Agentを開始しない                     |
| T05 | サーバーreadinessが遅れる                       | E2Eを準備完了まで開始しない                       |
| T06 | readiness前にプロセス終了                       | 待機を終了して失敗を記録                          |
| T07 | 別プロセスがポート使用中                        | 誤ってreadyにせず、起動を保留・失敗にする         |
| T08 | serviceがready後に終了                          | 依存側に設定した停止規則を適用                    |
| T09 | 複数serviceと後続job                            | service数がjob用実行枠を塞がない                  |
| T10 | Agentがidleになる                               | job成功へ自動変換しない                           |
| T11 | 同じ作成イベントが重複                          | 同じ初期化を並行して開始しない                    |
| T12 | 自分のupから作成イベント受信                    | 別Runを増やさない                                 |
| T13 | Git作成成功・応答保存前の停止                   | 実状態を照合し、worktreeを増殖させない            |
| T14 | 子プロセス起動直後の停止                        | 状態不明を保持し、無断再実行しない                |
| T15 | Coordinatorの再起動                             | 既存Runnerと結果を照合し、稼働jobを重複起動しない |
| T16 | 古いAttemptの遅延通知                           | 新しいAttemptの状態を上書きしない                 |
| T17 | Herdr接続切断・再接続                           | 古いpane IDを照合せず操作しない                   |
| T18 | CLI作成とTUI作成                                | 各対象Herdr版で同じ適用方針になる                 |
| T19 | grid重複、範囲外、分割不能                      | 配置開始前に拒否                                  |
| T20 | 利用者が既存ペインを変更                        | resumeでそのペインを置換しない                    |
| T21 | ログペインを閉じて再表示                        | 実行中serviceを再起動しない                       |
| T22 | コピー先が追跡済み、パス脱出、symlink           | ファイルを変更せず拒否                            |
| T23 | コピー途中の失敗                                | 記録された変更だけを復旧し、復旧不能分を示す      |
| T24 | 未承認リポジトリの作成イベント                  | 自動コマンドを実行しない                          |
| T25 | 設定変更後の自動実行                            | 古い承認で実行しない                              |
| T26 | stop時に子孫プロセスが残る                      | 対象を照合して停止し、残存分を報告                |
| T27 | dirty / untracked / ignoredなファイルがある削除 | 内容を確認せず削除しない                          |
| T28 | 外部からworktreeが消える                        | 削除済みパスを作り直さず、状態を更新              |
| T29 | ログディスク満杯、DB書き込み失敗                | 劣化を通知し、新規起動を安全側に止める            |
| T30 | Agent用プロンプトの送信応答を失う               | 同じプロンプトを無断再送しない                    |
| T31 | 同一checkoutを別Herdrセッションから操作         | checkoutロックで初期化を直列化                    |
| T32 | Linux、macOS、WSL                               | パス、信号、ログ、通信の契約試験を通過            |

設定例は意味検証の固定テストにする。プロセス試験には制御可能な小さなHTTPサーバー、遅延終了、大量ログ、子孫プロセスを作るfixtureを使用する。実エージェントを起動する試験は明示的な有効化制とし、通常のCIで利用者の認証や課金を使わない。

## 25. 設計上の保留事項

| 論点                             | 現時点の方針                                       |
| -------------------------------- | -------------------------------------------------- |
| 公開名とplugin ID                | `herdr-workflow`は仮称。公開前に重複を調査         |
| Herdr対応下限                    | 必要APIの実機確認後に決定                          |
| 任意Task一覧のサイドバー表示     | 初期版は端末画面。ネイティブ追加は未確認           |
| 対話RunnerのPTYと終了管理        | 初期の非対話実行から分離して検証                   |
| Windowsネイティブ                | named pipeとJob Object等の実装後に対応判定         |
| 複数worktreeをまたぐ親DAG        | 一つのRunと一つのworktreeの仕様が安定した後に追加  |
| 複数の実行結果をまたぐキャッシュ | 初期版では提供しない                               |
| branch別profileと外部プラグイン  | 実行完了を確認できる連携から段階的に追加           |
| 自動承認の詳細粒度               | 設定ハッシュ単位を起点に、許可項目を実装時に定義   |
| 設定上限と性能値                 | 実装時の測定で決める。未測定の応答時間を保証しない |

## 26. これまでの案から変更した点

前回の構成案にあった`serde_yaml`は、保守終了のため採用候補から外した。[S5]

worktree作成後に初期化し、その後でHerdr workspaceを作るという説明は修正した。Herdrの通常経路ではworkspaceが先に存在するため、初期化後に追加するのは主に管理用タブとペインである。[S1]

通常コマンドの状態が、そのままHerdrのAgentsサイドバーに表示されるとは扱わない。プラグインのTask状態、Agent観測状態、表示機能を分けた。

長時間タスクと再開を扱うため、Herdr hookへ処理を直接詰め込む案から、CoordinatorとTaskRunnerを持つ構成へ変更した。また、タスクの起動成功、準備完了、終了成功を異なる条件として定義した。

## 27. 参考資料

公開資料の確認日は2026-09-11。ここに挙げた仕様を利用者のインストール済みバージョンで確認したわけではない。実装時は参照する版を固定し、実機試験の結果を対応表へ追加する。

| ID  | 資料                                 | 参照先                                                           |
| --- | ------------------------------------ | ---------------------------------------------------------------- |
| S1  | Herdr Socket API                     | `https://herdr.dev/docs/socket-api/`                             |
| S2  | Herdr Plugins                        | `https://herdr.dev/docs/plugins/`                                |
| S3  | Herdr Worktree Bootstrap・作者README | `https://github.com/zerodice0/herdr-plugin-worktree-bootstrap`   |
| S4  | Herdr Workspace Manager・作者README  | `https://github.com/razajamil/herdr-plugin-workspace-manager`    |
| S5  | serde_yaml・保守状況                 | `https://docs.rs/serde_yaml/latest/serde_yaml/`                  |
| S6  | serde-saphyr・API資料                | `https://docs.rs/serde-saphyr/latest/serde_saphyr/`              |
| S7  | Tokio process・終了処理の注意事項    | `https://docs.rs/tokio/latest/tokio/process/`                    |
| S8  | mise exec                            | `https://mise.jdx.dev/cli/exec.html`                             |
| S9  | Git worktree                         | `https://git-scm.com/docs/git-worktree`                          |
| S10 | Git hooks                            | `https://git-scm.com/docs/githooks`                              |
| S11 | mise trust                           | `https://mise.jdx.dev/cli/trust.html`                            |
| S12 | petgraph toposort                    | `https://docs.rs/petgraph/latest/petgraph/algo/fn.toposort.html` |
| S13 | rusqlite                             | `https://docs.rs/rusqlite/latest/rusqlite/`                      |
| S14 | Herdr Windows beta                   | `https://herdr.dev/docs/windows-beta/`                           |

---

## 付録：最初に固定する設計契約

最初の実装着手点は、`WorkflowSpec`、`ExecutionPlan`、`TaskState`、`AttemptResult`の四つの型と、それを検証するテストとする。

設定例から計画を生成し、「初期化失敗時はAgentを起動しない」「serviceのready後にE2Eを実行する」「再開時に既存タスクを二重起動しない」「resumeで既存タブを置換しない」をテストで固定する。その後にHerdrAdapterを接続する。
