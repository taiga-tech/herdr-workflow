---
id: REF-CONFIGURATION
title: '設定リファレンス'
status: draft
documentVersion: '0.2'
updated: '2026-11-05'
---

# 設定リファレンス

[文書目次](../README.md) · [更新ルール](../maintenance.md)

> statusは`draft`です。実装済みとして扱う範囲は、コードと試験結果で確認します。

この文書の管理対象：workflow設定とプラグイン全体設定の読み込み、キー表記、値置換、設定例への案内。

<a id="source-06"></a>

## 設定ファイルの区分

Herdr Workflowは、リポジトリが所有するworkflow設定と、利用者が所有するプラグイン全体設定を分ける。

| 区分               | パス                                                     | 所有者     | 用途                                                       |
| ------------------ | -------------------------------------------------------- | ---------- | ---------------------------------------------------------- |
| workflow設定       | 明示した`--config`、または作成元の`.herdr/workflow.yaml` | リポジトリ | Task、DAG、worktree、workspace等の実行定義                 |
| プラグイン全体設定 | `$HERDR_PLUGIN_CONFIG_DIR/config.yaml`                   | 利用者     | 全workflowへ適用する運用上限と、今後追加する利用者固有設定 |
| 秘密値の定義       | `$HERDR_PLUGIN_CONFIG_DIR/secrets.yaml`                  | 利用者     | workflow設定の`{secret: <名前>}`が参照する値               |

workflow設定からプラグイン全体設定を上書きできない。プラグイン全体設定と承認情報をリポジトリへコミットしない。[ADR-0006](../decisions/0006-plugin-config.md)

## workflow設定

保存先の案は`.herdr/workflow.yaml`とする。参照先はNext.jsとPrismaを使うプロジェクトの例である。`mise.toml`、Prisma schema、`test:e2e`スクリプトが対象プロジェクトに存在する前提であり、プラグインが自動検出して追加するものではない。

設定全体は[examples/workflow.yaml](../../examples/workflow.yaml)を参照する。この文書には全文を複製しない。

`mise exec -- ...`を経由し、対象worktreeのmise設定で後続コマンドを実行する。`mise install`後に、親プロセスのPATHが自動更新されたと仮定しない。[S8](sources.md#s8)

この例の`PORT`は実行ごとに変更できる。並行する別Runへ同じポートを指定した場合は、利用中であることを検出して起動を止める。空きポートの検査と実際のbindの間には競合余地があるため、ポート番号の確保を検査だけで保証しない。

### 設定の規則

設定キーはcamelCaseとし、未知のキーと重複キーをエラーにする。`version`は設定スキーマの版であり、プラグインのリリース番号とは分ける。

Task IDとpane IDは明示する。表示名は識別子として使わない。Agentタスクは一つのAgent用ペインへ対応づける。ログ表示は一つのタスクに複数置いても、プロセスを再起動しない。

コマンドは`argv`を標準形式とする。引数に空白が含まれていても一つの要素として扱う。シェル構文が必要な場合だけ、実行シェルとscriptを別項目で明示する。`argv`とscriptの同時指定は拒否する。

`${inputs.port}`等はプラグインが定義した値置換であり、シェル評価ではない。未定義の変数、型不一致、パス制約違反は実行前にエラーにする。引数配列は置換後も分割しない。

YAMLの外部ファイル読み込み、任意のカスタムタグ、暗黙の実行式は初期版で扱わない。ファイルサイズ、深さ、alias展開量に上限を設定する。

### 読み込み元と承認

明示した`--config`を優先し、それ以外は作成元の`.herdr/workflow.yaml`を使う。

Run開始時に設定を解決し、ハッシュ付きの実行計画として固定する。worktree作成後に別ブランチの同名設定へ自動的に読み替えない。

## プラグイン全体設定

`HERDR_PLUGIN_CONFIG_DIR`が空でない場合は、その直下の`config.yaml`を読む。環境変数が未設定または空の場合と、`config.yaml`が存在しない場合は組み込み既定値を使う。ファイルが存在するのに読み取れない場合、YAMLが不正な場合、未知キーまたは重複キーを含む場合、未対応の`version`の場合、値が許容範囲外の場合は、workflow設定の検証前に設定エラーとして終了する。異常なファイルを既定値へ黙って置き換えない。

設定例の編集元は[examples/config.yaml](../../examples/config.yaml)とする。初期スキーマは次の構造を持つ。

```yaml
version: 1
limits:
    layout:
        maxGridDimension: 64
        maxPanesPerTab: 32
logs:
    maxFileBytes: 20971520
    generations: 5
```

`version`はプラグイン全体設定のスキーマ版であり、workflow設定の`version`およびプラグインのリリース番号とは別に管理する。設定キーはcamelCaseとする。将来の設定は責務ごとのトップレベル領域へ追加し、無関係な値を`limits`へ混在させない。

### プラグイン全体設定のキー

| キー                             | 型   | 省略時             | 許容範囲     | 意味                                                                                |
| -------------------------------- | ---- | ------------------ | ------------ | ----------------------------------------------------------------------------------- |
| `version`                        | 整数 | 省略不可           | `1`          | プラグイン全体設定のスキーマ版                                                      |
| `limits`                         | map  | 各組み込み既定値   | ―            | 資源消費を制限する設定群                                                            |
| `limits.layout`                  | map  | 各組み込み既定値   | ―            | レイアウト検証の上限                                                                |
| `limits.layout.maxGridDimension` | 整数 | `64`               | `1..=64`     | 一つのgridで許可する`columns`と`rows`それぞれの最大値                               |
| `limits.layout.maxPanesPerTab`   | 整数 | `32`               | `1..=32`     | 一つのtabで許可するpane数の最大値                                                   |
| `logs`                           | map  | 各組み込み既定値   | ―            | Attemptログの大きさと保持                                                           |
| `logs.maxFileBytes`              | 整数 | `20971520`(20 MiB) | 1 MiB〜1 GiB | 1ファイルの上限。超えたら世代を回す                                                 |
| `logs.generations`               | 整数 | `5`                | `1..=20`     | 現在のファイルを含む世代数                                                          |
| `logs.retentionDays`             | 整数 | 削除しない         | `1..=3650`   | 終了・停止から指定日数を過ぎたRunのAttemptファイルを、Coordinatorの起動時に削除する |

`maxGridDimension`と`maxPanesPerTab`は、利用者がそれぞれ組み込み安全上限64と32を引き下げるための運用上限であり、設定によって安全上限を超える値へ緩和できない。workflow内のいずれかのgridで`columns`または`rows`が有効な`maxGridDimension`を超えた場合、またはtabのpane数が有効な`maxPanesPerTab`を超えた場合は、重複検査やペイン配置を開始する前に拒否する。上限を引き上げる場合は、grid面積、pane数、重複検査と境界探索の時間、およびHerdr側の実動作を測定してから契約を変更する。

`validate`と`plan`はプラグイン全体設定を読み、その有効値でworkflowを検証する。将来Runを開始するコマンドは開始時の有効なプラグイン全体設定も実行計画ハッシュへ含め、そのRunの途中で`config.yaml`を再読込して計画を変更しない。

## 設定領域の参照先

| 領域                                    | 記載内容                         | 動作規則                              |
| --------------------------------------- | -------------------------------- | ------------------------------------- |
| `inputs`                                | 入力の型、既定値、値置換         | この文書                              |
| `worktree` / `files`                    | checkout準備と許可ファイルコピー | [worktree仕様](../specs/worktree.md)  |
| `defaults` / `tasks`                    | command、Agent、実行環境         | [実行仕様](../specs/execution.md)     |
| `bootstrap` / `dependsOn` / `execution` | 初期化対象と依存条件、実行枠     | [DAG仕様](../specs/workflow.md)       |
| `waitFor`                               | HTTP、TCP、ログによる準備確認    | [観測仕様](../specs/observability.md) |
| `workspace` / `placement`               | tab、pane、grid                  | [配置仕様](../specs/layout.md)        |
| `cleanup`                               | 停止と保存の方針                 | [後片付け仕様](../specs/cleanup.md)   |

<a id="q11-keys"></a>

## Q11で定めた入力形式

未決事項[Q11](../planning/open-questions.md#q11)で決めたworkflow設定のキーを次に示す。動作規則はリンク先の仕様を主文書とする。これらのキーは設定型(`src/config/model.rs`)、意味検証(`src/config/validate.rs`・`src/config/substitute.rs`)、生成スキーマ(`schema/workflow.schema.json`)で扱う。`secrets.yaml`は`src/config/secrets.rs`で読み込む。

### commandタスク

| キー                 | 型                                 | 省略時                    | 制約                                               | 動作規則                                    |
| -------------------- | ---------------------------------- | ------------------------- | -------------------------------------------------- | ------------------------------------------- |
| `argv`               | 文字列の配列                       | `script`と排他で一方必須  | 空配列不可                                         | [実行](../specs/execution.md#script)        |
| `script.shell`       | 文字列の配列                       | 省略不可                  | 空配列不可。値置換を行わない                       | [実行](../specs/execution.md#script)        |
| `script.source`      | 文字列                             | 省略不可                  | 値置換を行わない                                   | [実行](../specs/execution.md#script)        |
| `env.<NAME>`         | 文字列、`null`、`{secret: <名前>}` | 起動元の値を継承          | `HERDR_`で始まる名前、空・`=`・NULを含む名前を拒否 | [実行](../specs/execution.md#environment)   |
| `locks`              | 名前の配列                         | `[]`                      | 名前は`[a-z0-9][a-z0-9-]*`。同じ名前の重複を拒否   | [実行](../specs/execution.md#resource-lock) |
| `retry`              | map                                | `execution.retries`に従う | `lifecycle: job`だけに指定できる                   | [DAG](../specs/workflow.md#retry)           |
| `retry.maxAttempts`  | 整数                               | 省略不可                  | `1..=10`。初回を含む総Attempt数                    | [DAG](../specs/workflow.md#retry)           |
| `retry.delaySeconds` | 整数                               | `0`                       | `0..=3600`                                         | [DAG](../specs/workflow.md#retry)           |

`defaults.env`も`env.<NAME>`と同じ値の形式と名前の制約を持つ。`execution.retries`は`retry`を持たないjobの再試行回数(初回を含まない)とし、`maxAttempts = retries + 1`として扱う。範囲は`0..=9`とする。

### waitFor

`waitFor`は`lifecycle: service`のcommandタスクに指定する。Agentタスクは`waitFor`を持たず、Herdrによる起動・入力準備の確認でreadyとなる。

| 種類          | キー                       | 型                        | 省略時       | 制約                                                                       |
| ------------- | -------------------------- | ------------------------- | ------------ | -------------------------------------------------------------------------- |
| `http`        | 既存のキー                 | ―                         | ―            | [設定例](../../examples/workflow.yaml)の`server`を参照                     |
| `tcp`         | `host`                     | 文字列                    | `127.0.0.1`  | loopback以外は明示した場合だけ使う                                         |
| `tcp`         | `port`                     | 整数または値置換          | 省略不可     | 置換後に`1..=65535`の整数                                                  |
| `tcp`         | `timeoutSeconds`           | 整数                      | 省略不可     | `1`以上                                                                    |
| `tcp`         | `intervalMilliseconds`     | 整数                      | 省略不可     | `1`以上                                                                    |
| `tcp`         | `consecutiveSuccesses`     | 整数                      | 省略不可     | `1`以上                                                                    |
| `log`         | `stream`                   | `stdout`・`stderr`・`any` | `any`        | ―                                                                          |
| `log`         | `pattern`                  | 文字列(正規表現)          | 省略不可     | Rust `regex`の構文。1024文字以下。検証時にコンパイルする。値置換を行わない |
| `log`         | `timeoutSeconds`           | 整数                      | 省略不可     | `1`以上                                                                    |
| `http`・`tcp` | `monitor.intervalSeconds`  | 整数                      | 継続確認なし | `1`以上                                                                    |
| `http`・`tcp` | `monitor.failureThreshold` | 整数                      | 継続確認なし | `1`以上                                                                    |

`monitor`は省略可能で、指定する場合は二つのキーを両方書く。`log`に`monitor`を書いた場合は拒否する。動作規則は[観測](../specs/observability.md#readiness-config)を参照する。

### worktree.mode: attach

`worktree.mode: attach`は既存checkoutへRunを関連づける。対象パスを設定へ書かず、`attach --checkout PATH`またはHerdrイベントのworktree情報から受け取る。`branch`は省略可能で、指定した場合は対象checkoutのブランチとの一致を検査する。`base`、`existingBranch`、`ifAlreadyCheckedOut`は指定できない。動作規則は[worktree](../specs/worktree.md#attach)を参照する。

### 秘密値の定義ファイル

`{secret: <名前>}`の実体は、利用者が所有する`$HERDR_PLUGIN_CONFIG_DIR/secrets.yaml`に置く。workflow設定とプラグイン全体設定(`config.yaml`)には秘密値を書かない。

```yaml
version: 1
secrets:
    db-url:
        value: 'postgres://...'
    api-token:
        fromEnv: MY_API_TOKEN
```

| キー                     | 型     | 制約                                                         |
| ------------------------ | ------ | ------------------------------------------------------------ |
| `version`                | 整数   | `1`                                                          |
| `secrets.<名前>`         | map    | 名前は`[a-z0-9][a-z0-9-]*`。`value`と`fromEnv`のどちらか一方 |
| `secrets.<名前>.value`   | 文字列 | 秘密値そのもの                                               |
| `secrets.<名前>.fromEnv` | 文字列 | Coordinatorの起動元環境変数の名前                            |

ファイルがない場合、または`HERDR_PLUGIN_CONFIG_DIR`が未設定の場合は、定義が空であるとみなす。Coordinatorが実行するRunでは、Runを要求したCLIの`HERDR_PLUGIN_CONFIG_DIR`を使う。秘密値を参照していなければ失敗しない。解析エラーは値を引用しうるため、行と列だけを報告する。ファイルの扱いと権限は[承認と秘密情報](../specs/security.md#secrets)を参照する。

## 仕様化の残件

Q11で定めたキーは実装済みである。`stop.graceSeconds`を省略した場合の停止猶予は10秒とする。本文にないキーを推測して追加しない。

Rustの設定型(`WorkflowSpec`)から`herdr-workflow schema`コマンドでJSON Schema(`schema/workflow.schema.json`)を生成する。生成物と型を別々に手作業で更新しない。構造検証だけでは、循環依存やgridの分割可能性を検証したことにはならない。この役割分担は`tests/schema_contract.rs`で固定している(構造的に正しいがDAGとして不正な設定がスキーマ検証を通ることを確認し、その意味検証は`plan::compile::compile`が担う)。

## 関連文書

[workflow設定例](../../examples/workflow.yaml) / [プラグイン全体設定例](../../examples/config.yaml) / [未決事項](../planning/open-questions.md) / [workflow設定定義の判断](../decisions/0005-configuration-source.md) / [プラグイン全体設定の判断](../decisions/0006-plugin-config.md)
