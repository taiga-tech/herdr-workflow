# 作業計画

## ブランチレビュー（2026-09-14）

- 対象: `develop...HEAD`。設定・計画・CLIの不具合と回帰を確認し、製品コードは変更しない。
- [x] 比較元、差分、関連仕様と教訓を確認する
- [x] 設定検証・DAG・配置・CLIを分担してレビューする
- [x] 既存テストと指摘候補の再現を確認する
- [x] 根拠付きの指摘と検証範囲を報告する

### レビュー結果

- 未定義bootstrap targetとゼロspanでのpanic、初期化のstarted依存による進行不能、起動失敗Attemptによるstarted条件の誤成立、コピー先の表記差による重複検査漏れを再現した。
- Rustテスト100件、rust:fmt、rust:clippy、docs:test（3件）、docs:checkが成功した。Herdr実機・実Agent起動は検査していない。
- 製品コードは変更せず、レビュー記録のみを更新した。

## PR #3レビュー対応（2026-09-16）

- `condition_met`のStarted判定が`AttemptOutcome::LaunchFailed`でも成立していた不具合（上記「起動失敗Attemptによるstarted条件の誤成立」）を修正した(`src/plan/schedule.rs`)。回帰テストを`src/plan/schedule/tests.rs`に追加。
- `compute_init_boundary`内の到達不能な`UnknownBootstrapTarget`分岐を削除した(`src/plan/compile.rs`)。bootstrap targetと依存先は呼び出し前に存在確認済みのため。
- `validate_grid`の`col_start > columns` / `row_start > rows` / `col_start < 1` / `row_start < 1`チェックを、`resolve_panes`が保証する不変条件（col_start/row_start >= 1、col_end >= col_start、row_end >= row_start）により冗長と判断し、`col_end > columns` / `row_end > rows`のみへ簡略化した(`src/plan/layout.rs`)。

## 仕様

- このリポジトリを、設計文書の配布物ではなく Herdr Workflow プラグイン本体の開発リポジトリとして扱えるようにする。
- 現在が未実装である事実は維持しつつ、「別の実装用リポジトリへ移す」という前提を除く。
- AI 作業者向け案内を、文書編集だけでなく Rust 実装、テスト、設定例、文書同期を扱う内容へ更新する。
- コード変更に応じて主文書、設定例、ADR、受け入れ試験を同期する、リポジトリ内のドキュメント更新スキルを追加する。
- 永続参照用の現行文書から、特定の作業回や移管作業に依存する履歴的な記述を除き、現在の仕様・状態・保守手順として記述する。
- 既存の文書 ID、機能 ID、試験 ID、ADR 番号、`archive/` の元資料は維持する。

## 計画

- [x] 現在の文書専用・配布物前提の記述と、開発リポジトリ化に伴う整合箇所を特定する
- [x] `AGENTS.md` と入口文書を、実装開発を含む運用へ最小限の変更で更新する
- [x] 現行文書の作業履歴的な表現を、永続的な参照情報へ置き換える
- [x] ドキュメント更新スキルをリポジトリ内に作成し、利用条件と検証手順を定義する
- [x] 文書検査、スキル検査、差分確認を実行する
- [x] 検証結果と残課題をこのファイルのレビューへ記録する

## レビュー

- このリポジトリをRust実装、テスト、設定例、文書を同じ履歴で管理する開発リポジトリとして定義した。
- 現行文書から一回限りの配布・移管・作業報告表現を除き、履歴と監査資料を専用領域へ分離した。
- `.agents/skills/herdr-workflow-docs/`を追加し、現在の挙動を根拠付きで主文書へ同期する手順を定義した。
- 通常の文書検査から`.agents/`、`tasks/`、`archived`文書を分離し、`--check-migration`で監査資料を検査できるようにした。
- 文書検査モードの境界を固定する回帰テスト3件を追加した。
- 回帰テスト、通常検査、移行監査、Prettier、スキルバリデーター、`git diff --check`が成功し、独立レビューで重大・警告事項がないことを確認した。Rust実装、Herdr動作、実Agent起動は対象が存在しないため実行していない。

## 追加対応：文書用スクリプトの配置

- [x] 文書検査本体と回帰テストを`herdr-workflow-docs`スキル配下へ移動する
- [x] リポジトリルートの解決とテストのimportを新しい配置に合わせる
- [x] AGENTS、README、開発ガイド、文書更新ルール、構成図、スキル内のコマンドを更新する
- [x] 回帰テスト、通常検査、移行監査、スキル検査、整形、差分検査を再実行する

文書関連スクリプトを`.agents/skills/herdr-workflow-docs/scripts/`へ集約した。新しい配置から回帰テスト3件、通常検査、移行監査、スキル検査、Prettier、`git diff --check`が成功し、独立レビューでも重大・警告事項がないことを確認した。

## 追加対応：Pythonスクリプトのuv実行

- [x] `mise.toml`で文書検査に使うuvのバージョンを固定する
- [x] 文書とスキルに記載したPythonスクリプトの実行例をuv経由へ統一する
- [x] uv経由で回帰テスト、通常検査、移行監査、スキル検査を実行する
- [x] 整形と差分検査を行い、結果をレビューへ記録する

uv `0.12.11`を`mise.toml`で固定し、現行の実行例を`uv run --no-project python -B`へ統一した。uv経由の回帰テスト3件、通常検査、移行監査、スキル検査、Prettier、`git diff --check`が成功した。`python -B`により`__pycache__`が残らないことを確認し、独立レビューでも重大・警告事項はなかった。

## 追加対応：文書検査のmiseタスク化

- [x] uv経由の回帰テスト、通常検査、移行監査を`mise.toml`のタスクとして定義する
- [x] 文書とスキルの実行例を`mise run`へ統一する
- [x] 定義したmiseタスク、スキル検査、整形、差分検査を実行する
- [x] 独立レビュー後、今回のuv・mise対応をコミットする

`docs:test`、`docs:check`、`docs:audit`を追加し、各タスクから固定版uvで文書用Pythonスクリプトを実行する構成にした。現行文書とスキルは`mise run`を共通の入口とする。3タスク、スキル検査、Prettier、`git diff --check`が成功し、`__pycache__`などの生成物がないことを確認した。独立レビューで指摘された更新日とタスク記録も修正した。

## 追加調査：不要ファイル監査

- [x] 既存の教訓、文書目次、リポジトリ構成、作業ツリー状態を確認する
- [x] 未追跡・ignored・生成物・一時ファイルの有無を確認する
- [x] 追跡済みファイルの役割、参照関係、重複・陳腐化候補を確認する
- [x] 独立調査結果と検査結果を照合し、削除候補と保持対象を分類する
- [x] 調査結果と未実施事項をレビューへ記録する

生成物、キャッシュ、未追跡・ignoredファイルは存在しなかった。ドキュメントに対象を絞ると、`herdr-workflow-design.md`は固有情報を持たない旧パス案内であり、外部リンク互換が不要なら削除候補。`docs/meta/validation-report.md`と`validation-result.json`は一回限りの検査記録であり、現行文書から除外する候補。調査時点では、後続の全体整形で保存資料2件が改変されていたため、`docs:audit`と`docs:test`がハッシュ不一致を検出した。`archive/`と残る`docs/meta/`は元文書移行の証跡として一体であり、移行監査を恒常運用しない方針なら、`docs:audit`、manifest、目次、関連リンクとまとめて整理できる。削除や置換は行っていない。

## 追加対応：不要文書の削除

- [x] 固有情報を持たない旧パス案内と、陳腐化した一回限りの検査結果を削除する
- [x] 文書目次、manifest、構成図、移行表、変更履歴から削除対象への参照を除く
- [x] 過去の全体整形で改変された元資料を記録済み原本へ復元する
- [x] ユーザーの指示に従い、`.prettierignore`と`mise format`の設定を変更対象から外す
- [x] 文書回帰試験、通常検査、移行監査、整形、差分検査を実行する
- [x] 独立レビューの指摘に合わせ、旧パス互換と一回限りの検査結果の保管方針を明文化する
- [x] 修正後の独立再レビューを行い、結果と残課題を記録する

`herdr-workflow-design.md`、`docs/meta/validation-report.md`、`docs/meta/validation-result.json`を削除し、目次、manifest、構成図、移行表、変更履歴の参照を同期した。旧パスは外部参照が確認された場合だけ保持し、一回限りの検査結果はCI、作業記録、Git履歴で確認する方針へ更新した。過去の全体整形で改変された`archive`の元資料2件は記録済み原本へ復元した。ユーザー指示に従い`.prettierignore`は追加せず、`mise format`も変更していない。`docs:test`、`docs:check`、`docs:audit`、変更文書のPrettier検査、`git diff --check`が成功し、独立再レビューでCritical・Warningともになかった。

## 追加対応：コミット

- [x] 今回の変更だけであることを差分と状態から確認する
- [x] 対象ファイルをステージし、ステージ済み差分を確認する
- [x] 検証結果を維持した状態でコミットし、commitと作業ツリーを確認する

不要文書の削除、参照と運用方針の同期、保存資料の原本復元、作業記録を一つの文書整理コミットとして扱う。未追跡の`.claude/plans/`は今回の変更ではないため、ステージとコミットの対象外とする。

## 追加対応：formatのコミット

- [x] format差分が`archive`の保存資料2件だけであることを確認する
- [x] 整形後の内容に合わせて保存manifestのSHA-256を更新する
- [x] 対象差分だけをステージし、ステージ済みスナップショットで文書検査を行う
- [x] formatコミットを作成し、commitと残る作業ツリーを確認する

`archive`のMarkdownとYAMLのformat差分を採用し、`source-manifest.json`の記録日とSHA-256を更新する。未追跡の`.claude/plans/`は対象外とする。

## 実装：WorkflowSpec / ExecutionPlan / TaskState / AttemptResult

docs/architecture.md 77-81行目の設計契約(最初の実装着手点)を満たす。計画詳細は`.claude/plans/vivid-swinging-toucan.md`。

### 計画

- [x] `Cargo.toml`に`serde_yaml_ng`を追加し`cargo build --locked`を通す
- [x] `src/config/model.rs`を実装し、`examples/workflow.yaml`と目視で突き合わせる
- [x] `src/config/load.rs`を実装する(YAML読み込みのみ、意味検証はしない)
- [x] `src/plan/graph.rs`を実装しトポロジカルソート・循環検出のテストを書く
- [x] `src/plan/compile.rs`を実装し`ExecutionPlan`とcompileエラーのテストを書く(T02/T03対応含む)
- [x] `src/state/models.rs`を実装し`TaskState`/`AttemptResult`のテストを書く
- [x] `src/plan/schedule.rs`を実装し、4つの固定動作(初期化失敗時Agent起動抑止/service ready後のE2E/再開時二重起動防止/resumeでタブ非置換)をテストで固定する
- [x] `tests/workflow_contract.rs`で設定例の実読み込み+compileを固定する(T01対応含む)
- [x] `src/lib.rs`のmod宣言を更新する
- [x] `mise run rust:fmt` / `rust:clippy` / `rust:test`を通す
- [x] `herdr-workflow-docs`スキルで関連文書(architecture.md、repository-structure.md、ADR-0005、workflow.md、open-questions.md)を同期する
- [x] `mise run docs:test` / `docs:check`を通す
- [x] 検証結果と残課題をレビューへ記録する

### レビュー

- `WorkflowSpec`/`ExecutionPlan`/`TaskState`/`AttemptResult`の4型と、`src/config/model.rs`・`src/config/load.rs`・`src/plan/graph.rs`・`src/plan/compile.rs`・`src/plan/schedule.rs`・`src/state/models.rs`を実装した。`examples/workflow.yaml`は`serde_yaml_ng`で実読み込みでき、`WorkflowSpec` -> `ExecutionPlan`のcompileが通る。
- 契約が要求する4つの固定動作(初期化失敗時のAgent起動抑止、serviceのready後のE2E実行、再開時の二重起動防止、resumeでのタブ非置換)を`src/plan/schedule.rs`の純粋関数`runnable_tasks`/`tabs_to_create`とそのテストで固定した。
- T01(未知キー拒否)、T02(循環依存)、T03(service/agentへのsucceeded依存・jobへのready依存の拒否)に対応するテストも合わせて追加した。
- 新規依存として`serde_yaml_ng`をCargo.tomlへ追加した(`serde_yaml`はアーカイブ済みのため)。ADR-0005にこの決定を追記し、状態は`proposed`のまま維持した(値置換・重複キー検出等の他の採用条件は未解決のため)。
- `docs/architecture.md`(実装済み範囲の追記)、`docs/repository-structure.md`(実際の構成・`plan/schedule.rs`の反映)、`docs/planning/open-questions.md`(Q11に未実装バリアントを追記)、`docs/specs/workflow.md`(依存条件の拒否対象を明確化)を同期した。
- `cargo test --locked`(37件)、`cargo fmt --all -- --check`、`cargo clippy --locked --all-targets --all-features -- -D warnings`、`mise run docs:test`、`mise run docs:check`、`prettier --check`が全て成功した。Herdr実機動作・実Agent起動は対象が存在しないため未実施。
- 未対応のまま残した範囲(次段階):`config/validate.rs`(未定義変数・重複キー検出)、`plan/layout.rs`(grid変換)、`runtime/*`・`herdr/*`等のHerdrAdapter接続、`schema/workflow.schema.json`生成。

## 実装:段階A(計画器)の拡張

計画詳細は`.claude/plans/vivid-swinging-toucan.md`。出口条件「不正設定を外部変更前に拒否できる」に向け、設定の意味検証、grid変換、CLIを実装する。

### 計画

- [x] `src/config/model.rs`に`no_duplicate_map`ヘルパーを追加し、`inputs`/`tasks`/`defaults.env`/`Command.env`へ適用してテストを書く
- [x] `src/config/substitute.rs`を新規作成する(プレースホルダー検出、`inputs`解決、`template_fields`、`substitute_all`)
- [x] `src/config/validate.rs`を新規作成する(未定義変数・未知名前空間・default型不一致・ID重複・コピーパス制約の検証)
- [x] `src/plan/layout.rs`を新規作成する(grid検証+分割木変換、`layout_not_sliceable`判定)
- [x] `src/plan/compile.rs`に`validate()`呼び出しと`layout::compile_layout`を組み込み、`CompileError`とSerializeを拡張する
- [x] `tests/workflow_contract.rs`に統合テスト(layoutスライス確認、重複キー、未定義変数、ピンホイール拒否)を追加する
- [x] `Cargo.toml`に`sha2`を追加する
- [x] `src/cli.rs`を新規作成する(`validate`/`plan`サブコマンド、ハッシュ表示、`--json`出力)
- [x] `src/main.rs`を書き換える
- [x] `mise run rust:fmt` / `rust:clippy` / `rust:test`を通す
- [x] `cargo run -- validate`と`cargo run -- plan`を手動実行して動作確認する
- [x] `herdr-workflow-docs`スキルで関連文書(architecture.md、repository-structure.md、roadmap.md、open-questions.md、testing/acceptance.md)を同期する
- [x] `mise run docs:test` / `docs:check`を通す
- [x] 検証結果と残課題をレビューへ記録する

### レビュー

- 段階A(計画器)の出口条件「不正設定を外部変更前に拒否できる」に向け、意味検証の完成・grid変換・CLIを実装した。
    - `src/config/model.rs`: `no_duplicate_map`ヘルパーで`inputs`/`tasks`/`defaults.env`/`Command.env`の重複キーを検出(固定フィールド構造体はserde-deriveが標準で検出済みと判明したため対象は4フィールドのみ)。
    - `src/config/substitute.rs`(新規): `${inputs.x}`のプレースホルダー検出・型検証・解決。`${worktree.path}`等の実行時変数は予約名前空間として構文チェックのみ行い値は解決しない。
    - `src/config/validate.rs`(新規): 未定義変数、未知名前空間、input default型不一致、tab/pane ID重複、files.copyの絶対パス・`..`脱出・宛先重複を検証(実ファイルシステムは見ない)。
    - `src/plan/layout.rs`(新規): gridの検証(範囲外・重なり・未配置セル)と二分割木への変換。examples/workflow.yamlの配置がスライス可能であることをテストで確認、非スライス例(3x3ピンホイール)で`NotSliceable`を確認。
    - `src/plan/compile.rs`: `validate()`と`layout::compile_layout()`を`compile()`に統合。JSON出力用に主要な型へSerializeを追加。
    - `src/cli.rs`(新規)・`src/main.rs`: `validate`/`plan`サブコマンド。`plan`はSHA-256の計画ハッシュと解決済み入力・値を`--json`で出力する。新規依存`sha2`を追加。CLIパーサーは`clap`を使わず手書き(サブコマンド2つのみのため)。
- テストは単体81件+統合6件の計87件全通過。T01(未知キー・重複キー・未定義変数)・T19(grid重複・範囲外・分割不能)に対応するテストを追加した。
- `cargo fmt --all -- --check`、`cargo clippy --locked --all-targets --all-features -- -D warnings`、`cargo run -- validate`/`plan`の手動実行、`mise run docs:test`/`docs:check`、`prettier --check`が全て成功した。
- 関連文書(architecture.md、repository-structure.md、roadmap.md)を同期した。`docs/testing/acceptance.md`は検証状況を記録する列がまだ存在せず、一回限りの検査結果を永続文書へ書き込むことになるため、意図的に変更しなかった(進捗はroadmap.mdの「段階Aの進行状況」節に記録)。`open-questions.md`は、今回の残課題が「未確定の契約」ではなく「確定済み契約の未実装部分」と判断し追記しなかった。
- 段階Aは完了扱いにしていない。未解消:並行実行設定値(`execution.maxConcurrentJobs`等)の意味検証、`files.copy`の追跡済みファイル判定(実Git状態が必要、段階Bで対応)、JSON Schema生成、`herdr-plugin.toml`。

## 修正：`src/cli.rs`のrust-analyzer E0308診断

### 仕様

- `run_plan`、`error_output`、`plan_hash`は既存のCLI出力契約を変えず、それぞれ`String`を返す。
- rust-analyzerとリポジトリ標準のRust検査で型不一致が発生しない状態にする。
- 既存の段階A実装と未コミット差分を保持し、修正範囲を診断原因に限定する。

### 計画

- [x] 診断行、現在の差分、コンパイラ出力を照合して、保存済みコードの不具合かエディタ側の一時状態かを特定する
- [x] 再現する場合は戻り値を明示する最小修正と、CLI出力契約を固定する回帰テストを追加する(保存済みコードでは再現せず、コード・テスト変更不要と確認)
- [x] `mise run rust:fmt`、`rust:clippy`、`rust:test`を実行する
- [x] `herdr-workflow-docs`の手順で文書影響を確認し、必要な文書検査を実行する
- [x] 独立レビューを行い、検証結果と残課題をレビューへ記録する

### レビュー

- 保存済み`src/cli.rs`では、`run_plan`のJSON/通常出力、`error_output`のJSON/通常出力、`plan_hash`の最終式がすべて`String`を返しており、提示されたE0308は再現しなかった。`cargo check --locked --all-targets --all-features`も成功した。
- VS Code拡張に同梱されたrust-analyzer 0.4.3047 (`6aeeb8cf02 2026-09-11`)で`analysis-stats .`を実行し、型推論の不明型0件、panic 0件、MIR lowering失敗0件を確認した。提示された診断は未保存の旧バッファ、またはlanguage serverに残った診断である可能性が高い。ただし、報告時のLSP overlay自体は観測できていない。
- 正常なCLI実装へ不要な`return`等を加えず、製品コードと主文書は変更しなかった。まずファイルを保存し、診断が残る場合は`Rust Analyzer: Restart server`を実行して現在の保存内容で再計算する。
- `mise run rust:fmt`、`mise run rust:clippy`、`mise run rust:test`(単体81件+統合6件)、`mise run docs:test`(3件)、`mise run docs:check`、対象作業記録のPrettier検査、`git diff --check`が成功した。CLI挙動を変更していないため、CLIの手動実行は省略した。
- 独立レビューはCritical 0、Warning 2だった。原因表現の確度と未実施のCLI手動確認に関する2点を修正した。
- 修正後の独立再レビューは、未解消のCritical 0、Warning 0だった。

## リファクタリング：大規模unit test moduleの分割

### 仕様

- `plan::schedule`と`plan::compile`のテストを、それぞれの親module配下の別ファイルへ移す。
- unit testのままprivate itemへアクセスできる構造を維持し、製品APIのvisibilityと挙動を変更しない。
- テスト名、検証内容、テスト件数を維持し、統合試験`tests/workflow_contract.rs`は変更しない。
- 現在のステージ済み実装を保持し、テスト配置の変更だけを追加する。

### 計画

- [x] `src/plan/schedule.rs`のinline test moduleを`src/plan/schedule/tests.rs`へ切り出す
- [x] `src/plan/compile.rs`のinline test moduleを`src/plan/compile/tests.rs`へ切り出す
- [x] test一覧と差分を比較し、件数・名前・製品コードが変わっていないことを確認する
- [x] `mise run rust:fmt`、`rust:clippy`、`rust:test`を実行する
- [x] `herdr-workflow-docs`の手順で文書影響を確認し、必要な文書と検査を同期する
- [x] 独立レビューを実施し、結果をレビューへ記録する

### レビュー

- `plan::schedule`のunit test 10件を`src/plan/schedule/tests.rs`へ、`plan::compile`のunit test 12件を`src/plan/compile/tests.rs`へ移し、親moduleには`#[cfg(test)] mod tests;`だけを残した。テストは親moduleの子であるため、`use super::*`によるprivate itemへのアクセスを維持し、製品APIのvisibilityは変更していない。
- ステージ済みのinline版と比較し、製品コードが不変であること、テスト名・件数・検証ロジックが一致することを確認した。移動に加えて、ピンホイール配置を5枚から6枚へ変えるという誤ったコメントを、実際のP1-P5の5枚構成に合わせて修正した。
- `docs/repository-structure.md`の現在構成と実装予定構成へ、新しいunit testファイルの配置を反映した。動作仕様、受け入れ条件、ADRに変更はない。
- `mise run rust:fmt`、`mise run rust:clippy`、`mise run rust:test`が成功し、単体81件と統合6件の計87件が通過した。`mise run docs:test`と`mise run docs:check`、対象文書のPrettier検査、`git diff --check`も成功した。Herdr実機動作・実Agent起動は挙動変更がないため実施していない。
- 独立レビューはCritical 0、Warning 0、Suggestion 0で、修正必須事項はなかった。

## コミット：段階A実装とテスト分割

### 計画

- [x] 設定値置換・意味検証を、設定moduleと単体試験のコミットへまとめる
- [x] grid検証・分割木変換・ExecutionPlan統合・契約試験を、計画生成のコミットへまとめる
- [x] SHA-256計画ハッシュと`validate`/`plan`サブコマンドを、CLIのコミットへまとめる
- [x] `plan::compile`と`plan::schedule`のunit test移動だけを、リファクタリングのコミットへまとめる
- [x] 現在の実装状態とリポジトリ構成を、永続参照文書のコミットへまとめる
- [x] 作業計画・レビュー・教訓を、製品変更と分けた作業記録のコミットへまとめる
- [x] 各コミットの差分と順序を確認し、最終状態でRust・文書・整形検査を再実行する
- [x] 独立レビュー後、commit一覧、作業ツリー、push未実施を確認する

### レビュー

- 段階A拡張の実装(設定値置換・意味検証、grid変換、CLI)を、意図通り5つの目的別コミットに分割できていることを`git log --stat`で確認した:`13af086`(設定値の置換と意味検証)、`35fd705`(gridレイアウト変換)、`027b5fa`(validate/planサブコマンド)、`0da6d64`(単体テストのファイル分割リファクタリング)、`604c06f`(文書同期)。各コミットは製品コード・テスト・文書のいずれかに責務が閉じており、差分の混在はなかった。
- 残っていた`tasks/lessons.md`・`tasks/todo.md`の更新分を、製品変更と分離した作業記録のコミットとしてまとめた。
- 最終状態で`cargo fmt --all -- --check`、`cargo clippy --locked --all-targets --all-features -- -D warnings`、`cargo test --locked`(単体81件+統合6件の計87件)、`mise run docs:test`(3件)、`mise run docs:check`、`prettier --check`をすべて再実行し、成功を確認した。
- `git status --short`で作業ツリーがクリーンであること、`git log`で6コミット(段階Aの最初の設計契約1件+今回の5件)がブランチ`feature/workflow-core-contract`に積まれていること、`git log origin/develop..HEAD`相当の比較でリモートに未pushであることを確認した。

## 実装:並行実行設定値の意味検証(段階A残課題)

計画詳細は`.claude/plans/vivid-swinging-toucan.md`。`execution.maxConcurrentJobs`/`maxLiveServices`/`maxLiveAgents`が`Some(0)`の場合を拒否する。`files.copy`の追跡済みファイル判定・JSON Schema生成・`herdr-plugin.toml`は今回も対象外(前者は段階Bへ先送り、後2つは未確定仕様のため)。

### 計画

- [x] `src/config/validate.rs`に`ValidateError::ZeroExecutionLimit`と`validate_execution_limits`を追加し`validate()`から呼ぶ
- [x] `zero_max_concurrent_jobs_is_rejected` / `zero_max_live_services_is_rejected` / `zero_max_live_agents_is_rejected` / `none_execution_limits_are_accepted` / `retries_zero_is_accepted`のテストを追加する
- [x] `src/plan/compile.rs`に`compile_rejects_spec_with_zero_execution_limit`テストを追加する
- [x] `mise run rust:fmt` / `rust:clippy` / `rust:test`を通す
- [x] `cargo run -- validate --config examples/workflow.yaml`が引き続き成功することを確認する
- [x] `herdr-workflow-docs`スキルで`docs/planning/roadmap.md`の残課題記述を更新する
- [x] `mise run docs:test` / `docs:check` / `prettier --check`を通す
- [x] 検証結果と残課題をレビューへ記録する

### レビュー

- `src/config/validate.rs`に`validate_execution_limits`を追加し、`execution.maxConcurrentJobs`/`maxLiveServices`/`maxLiveAgents`が`Some(0)`の場合を`ValidateError::ZeroExecutionLimit`で拒否するようにした。`None`(制限なし)と`retries: 0`は正当な値として許容する。
- `src/plan/compile.rs`(`compile/tests.rs`)に`compile_rejects_spec_with_zero_execution_limit`を追加し、`compile()`経由でも`CompileError::Invalid`として伝播することを確認した。
- テストは単体87件+統合6件の計93件(6件増加)全通過。`cargo fmt`/`clippy -D warnings`/`cargo run -- validate`(既存examples/workflow.yamlは4/8/4のため無影響)がすべて成功した。
- `docs/planning/roadmap.md`の「段階Aの進行状況」を更新し、並行実行設定値の意味検証を解消済みとして本文に統合、残る未解消項目(`files.copy`の追跡済みファイル判定、JSON Schema生成、`herdr-plugin.toml`)だけを残した。`mise run docs:test`/`docs:check`/`prettier --check`成功。
- ユーザー確認済みの通り、`files.copy`の追跡済みファイル判定は段階Bへ先送り、JSON Schema生成と`herdr-plugin.toml`は仕様未確定のため今回も対象外とした。段階Aはこれらの解消後に完了とする。

## 調査:herdr-plugin.tomlの実機仕様確認

「本文にないキーを推測して追加しない」「Herdrに存在するAPIを推測せず、対象版の仕様または実機で確認する」というCLAUDE.mdの方針に従い、`herdr-plugin.toml`の仕様を実機のHerdr(v0.9.0、ローカルインストール済み)で確認した。

### 計画

- [x] `herdr --help`/`herdr integration --help`/`herdr api --help`で、プラグイン登録機構が実際に存在するか確認する
- [x] `herdr api schema --json`でSocket APIスキーマを取得し、`InstalledPluginInfo`および`PluginManifest*`系の型定義を抽出する
- [x] `herdr --skill`が案内する公式ドキュメント(v0.9.0タグのplugins.mdx)を取得し、実機スキーマとの整合を確認する
- [x] 確認結果を`docs/reference/cli.md`・`docs/integrations/herdr.md`・`docs/reference/sources.md`へ反映する
- [x] `mise run docs:test` / `docs:check` / `prettier --check`を通す

### レビュー

- `herdr integration`はAIエージェント(Claude/Codex等)へのフック統合管理であり、`herdr-workflow`のような外部ワークフロープラグインの登録機構とは別物と判明した。プラグイン登録は`herdr plugin link`/`herdr plugin install`(ヘルプ非表示だが公式docsに記載)で行う。
- `herdr api schema --json`から`herdr-plugin.toml`が実行時に`InstalledPluginInfo`型としてロードされることを確認した。必須フィールド`id`/`name`/`version`/`min_herdr_version`、セクション`[[actions]]`(id/title/command/contexts/description?/platforms?)/`[[panes]]`(id/title/command/placement?/width?/height?/description?/platforms?)/`[[startup]]`/`[[events]]`/`[[link_handlers]]`/`[[build]]`を実機スキーマとv0.9.0タグの公式ドキュメントの両方で確認し、内容が一致することを確かめた。
- `docs/reference/cli.md`のHerdr action節、`docs/integrations/herdr.md`の互換性の記録節、`docs/reference/sources.md`(確認した外部仕様表・S1/S2参照・外部仕様の再確認記録の新設表)を更新した。対応下限(`min_herdr_version`の具体的な値)は引き続き未決定のまま。
- ユーザー確認済みの通り、今回は文書反映のみに留め、`herdr-plugin.toml`実ファイルの作成は次回以降とした。
- `mise run docs:test`(3件)、`mise run docs:check`、`prettier --check`が成功した。一時的な調査ファイル(`/tmp/herdr_api_schema.json`)は削除済み。

## 実装:herdr-plugin.tomlの作成

前段の調査結果に基づき、`herdr-plugin.toml`をリポジトリルートに作成し実機検証する。

### 計画

- [x] `herdr plugin --help`でサブコマンド(`link`/`unlink`/`list`/`action`/`log`)を確認する
- [x] `herdr-plugin.toml`を作成する(`id`/`name`/`version`/`min_herdr_version`/`description`/`platforms`、`[[build]]`、`validate`のみの`[[actions]]`)
- [x] `cargo build --release`でバイナリを生成する
- [x] `herdr plugin link`で実機にリンクし、`action list`/`action invoke`/`plugin log`で成功時(exit 0)・失敗時(exit 2)の両方を確認する
- [x] 検証後`herdr plugin unlink`で環境を元に戻す
- [x] `docs/architecture.md`・`docs/planning/roadmap.md`・`docs/repository-structure.md`を更新する
- [x] `mise run rust:fmt` / `rust:clippy` / `rust:test`、`mise run docs:test` / `docs:check` / `prettier --check`を通す
- [x] 検証結果と残課題をレビューへ記録する

### レビュー

- `herdr-plugin.toml`を作成した。`id="herdr-workflow"`、`min_herdr_version="0.9.0"`(実機確認済みのバージョンに合わせた)、`platforms=["linux","macos"]`(Windowsは受け入れ試験T32が未実施のため対象外)。`[[build]]`は`cargo build --release`。`[[actions]]`には`validate`のみ登録し、`plan`は`--input`必須で単純呼び出しでは機能しないため見送った(段階Cで対話的な`[[panes]]`を実装する際に検討する)。
- 実機のHerdr v0.9.0で`herdr plugin link "$(pwd)"`→`herdr plugin action list`→`herdr plugin action invoke validate --plugin herdr-workflow`→`herdr plugin log`の手順で動作確認した。`.herdr/workflow.yaml`が存在しない状態では`exit_code=2`・`stdout`に読み込みエラーメッセージ、`examples/workflow.yaml`を一時的に`.herdr/workflow.yaml`へ配置した状態では`exit_code=0`・`stdout="設定は妥当です"`となり、両方とも期待通りだった。検証後`herdr plugin unlink herdr-workflow`で環境を元に戻し、一時検証用の`.herdr/`ディレクトリも削除した。
- `docs/architecture.md`(最初に固定する設計契約の続き)、`docs/planning/roadmap.md`(段階Aの進行状況、`herdr-plugin.toml`を残課題から除外)、`docs/repository-structure.md`(現在の状態・現在のリポジトリ構成ツリー)を実機検証結果に基づいて更新した。
- `cargo fmt`/`clippy -D warnings`/`cargo test`(単体87件+統合6件の計93件、変更なし)、`mise run docs:test`/`docs:check`/`prettier --check`がすべて成功した。
- 段階Aの未解消項目は`files.copy`の追跡済みファイル判定(段階Bへ先送り)とJSON Schema生成の2点のみに減った。
- 作業中に、このセッションで変更していない`mise.toml`の差分(`format`タスクの分割、`prettier:fmt`タスクの新設)がステージ済みの状態で見つかった。原因不明のため、コミット対象から分離しユーザーへ報告する。

## 実装:JSON Schema生成(段階Aの限界)

計画詳細は`.claude/plans/vivid-swinging-toucan.md`。`files.copy`の追跡済みファイル判定は実Git状態が必要で段階Bの領域のため対象外とし、JSON Schema生成のみを実装して段階Aの技術的に実装可能な範囲を完了させる。

### 計画

- [x] `cargo add schemars`(通常依存)、`cargo add jsonschema`(dev依存)を追加する
- [x] `src/config/model.rs`の全42公開型に`JsonSchema`deriveを追加し、既存serde属性(transparent/tag/untagged/deserialize_with)との組み合わせをビルドで確認する
- [x] `src/cli.rs`に`schema [--output PATH]`サブコマンドを追加しテストを書く
- [x] `cargo run -- schema --output schema/workflow.schema.json`で生成物を作成する
- [x] `tests/schema_contract.rs`を新規作成し、`jsonschema`で`examples/workflow.yaml`の妥当性を検証する
- [x] `docs/reference/cli.md`にコマンドを追記する
- [x] `docs/planning/roadmap.md`の段階Aの進行状況を更新する
- [x] `mise run rust:fmt` / `rust:clippy` / `rust:test`、`mise run docs:test` / `docs:check` / `prettier --check`を通す
- [x] 検証結果と残課題をレビューへ記録する

### レビュー

- `schemars`(v1.2.2、通常依存)と`jsonschema`(v0.56.0、dev依存)を追加した。`jsonschema`はreqwest等の推移的依存を多く引き込みビルド時間が伸びる(dev依存のみでテスト専用、本番`cargo build --release`には影響しない)。
- `src/config/model.rs`の全42公開型に`schemars::JsonSchema`をderiveした。懸念していた`#[serde(transparent)]`のnewtype、`#[serde(tag = "type")]`の内部タグ付きenum、`#[serde(untagged)]`のenum、`#[serde(deserialize_with = "no_duplicate_map")]`のフィールドはすべて追加コード無しでビルドが通り、生成されたJSON Schemaを目視確認したところ`oneOf`+`const`判別・`anyOf`・`additionalProperties`がそれぞれ正しく表現されていた。
- `src/cli.rs`に`schema [--output PATH]`サブコマンドを追加した(既存の共通フラグ解析ループへ`--output`を追加する形)。`--output`未指定時はstdoutへ、指定時はファイルへ書き込む。テスト4件(引数パース2件、stdout出力1件、ファイル書き込み1件)を追加した。
- `schema/workflow.schema.json`を生成しリポジトリに追跡した(`.gitignore`に`schema/`の除外はない)。
- `tests/schema_contract.rs`(新規)で、(1)`examples/workflow.yaml`が生成スキーマに対して妥当であること、(2)`version`に文字列を入れる等の構造違反がスキーマレベルで拒否されること、(3)循環依存を追加した設定が構造的にはスキーマを通過するが`plan::compile::compile`では拒否されること、の3点を確認し、「JSON Schemaは構造検証のみを担い、DAG・gridの意味検証はRust側が担う」というADR-0005の役割分担を実証した。
- `docs/reference/cli.md`(コマンド一覧・実行例)、`docs/reference/configuration.md`(仕様化の残件)、`docs/decisions/0005-configuration-source.md`(JSON Schema生成の実装を追記、Q11未解決のため状態は`proposed`のまま維持)、`docs/guides/development.md`(スキーマ出力の候補→採用済みに更新、かつ「Rust実装が存在しない」という古い記述を修正)、`docs/planning/roadmap.md`(段階Aとして技術的に実装可能な範囲が完了したことを記録)、`docs/repository-structure.md`(schema/・tests/schema_contract.rsの反映)、`docs/architecture.md`(最初に固定する設計契約の続き)を更新した。
- `cargo fmt`/`clippy -D warnings`/`cargo test`(単体91件+統合6件+schema_contract 3件の計100件)、`mise run docs:test`/`docs:check`/`prettier --check`がすべて成功した。
- 段階Aとして技術的に実装可能な範囲はこれで完了した。残る`files.copy`の追跡済みファイル判定は実worktreeのGit状態が必要なため、意図的に段階Bへ持ち越す。ADR-0005は「Q11の未定義キーを決める」が未解決のため`proposed`のまま。

## コミット: 段階A残課題の作業記録

### 計画

- [x] 未コミット差分が段階A残課題に関する作業記録だけであり、対応する製品変更が既存コミットに含まれることを確認する
- [x] 文書検査、対象Markdownの整形検査、差分検査を実行する
- [x] `tasks/lessons.md`と`tasks/todo.md`だけをステージし、ステージ済み差分を確認する
- [x] 作業記録を1コミットにまとめ、commit、作業ツリー、push未実施を確認する

### レビュー

- 未コミット差分は`tasks/lessons.md`と`tasks/todo.md`だけで、並行実行設定値の意味検証、Herdrプラグイン仕様の実機調査・manifest作成、JSON Schema生成に対応する作業記録と教訓だった。製品コード・主文書・依存関係の変更は既存コミットへ分離済みであることを確認した。
- `mise run docs:test`(3件)、`mise run docs:check`、対象2ファイルの`prettier --check`、`git diff --check`が成功した。独立レビューでもCritical・Warningはなく、作業記録として1コミットにまとめる判断が妥当と確認した。
- 対象2ファイルだけを`docs: 段階A残課題の作業記録と教訓を追記する`としてコミットし、pushは行わない。

## PR #3: レビュー8件への対応

### 仕様と計画

同一または古いAttempt結果を無視し、Unknownはstartedを満たさない。引数エラーも共通の出力形式を使う。ハッシュ入力と値を維持して中間JSON文字列を省き、タスクID・軸判定・入力エラー変換の重複を削減する。ADR採用基準は編集せず確認結果のみ報告する。コミットしない。

- [x] 現行仕様・コード・教訓と作業ツリーを確認する
- [x] 項目1・2・5の修正と回帰テスト
- [x] 項目3・6・8の修正と回帰テスト
- [x] 項目7の修正と回帰テスト
- [x] 文書同期と項目4の報告内容を確認する
- [x] Cargoの3検査、文書検査、差分レビューを完了する

### レビュー

- 項目1・2・3・5・6・7・8を修正し、日本語コメント付き単体回帰テストを7件追加した。計画JSONのタスクIDはマップキーに集約した。ハッシュ値は従来のJSON文字列経由と一致することを検査する。
- 項目4は報告のみ。developとの差分で「Rust採用だけをaccepted」から「利用者が採用を明示した判断」への変更とADR-0006のaccepted追加を確認した。承認の有無は判断せず、ADRは編集していない。
- 独立レビューで追加指摘なし。追加テストの型・書式のコンパイルエラーとClippyの不要参照を修正後、`cargo test`(単体132件・統合10件)、`cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`が成功した。CLIバイナリの引数エラーも3ケース確認した。
- `mise run docs:test`(5件)、`mise run docs:check`、変更Markdownの`prettier --check`、`git diff --check`が成功した。Herdr実機・実Agent起動は未実施。コミットは行っていない。
