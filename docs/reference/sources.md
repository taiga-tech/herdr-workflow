---
id: REF-SOURCES
title: '外部仕様と参考資料'
status: reference
documentVersion: '0.2'
updated: '2026-09-11'
---

# 外部仕様と参考資料

[文書目次](../README.md) · [更新ルール](../maintenance.md)

この文書の管理対象：外部資料、確認の範囲、確認時点。

## 資料の扱い

S1〜S14の公開資料は2026-09-11に確認した範囲を記載している。全リンク先を継続的に再検証しているという意味ではない。製品の互換性を主張するときは、対象版と実機試験を別に記録する。

D1〜D3は文書構成の根拠として2026-09-11に確認した一次資料である。分類、ADR、リンクの扱いを参照する。外部資料の内容から、このプラグインの実装済み機能を推定しない。

<a id="source-02"></a>

## 確認した外部仕様

| 確認事項                                                                                                                              | 設計への反映                                                                        |
| ------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------- |
| Herdrの`worktree.create`はcheckoutとHerdr workspaceをまとめて作成する。[S1](sources.md#s1)                                            | 作成後にworkspaceをもう一度作らない。返却されたIDを使う。                           |
| `layout.apply`は分割木からタブを作成する。既存タブを指定すると置換され、実行中プロセス等は維持されない。[S1](sources.md#s1)           | 通常の再開処理で既存タブを置換しない。                                              |
| 公開APIにはイベント購読、Agent操作、メタデータ報告、プラグイン用端末画面がある。[S1](sources.md#s1)[S2](sources.md#s2)                | 状態取得と表示に使う。任意のサイドバー部品追加は前提にしない。                      |
| Herdrのstartup hookは一回実行する初期化処理であり、daemonの監視機構ではない。[S2](sources.md#s2)                                      | hookは受付と起動確認だけで終了する。実行管理部の復旧はプラグインが扱う。            |
| `worktree-bootstrap`の作者は、作成イベント処理と別hookとの順序保証がないと説明している。[S3](sources.md#s3)                           | 初期化完了をDAGの依存条件にする。                                                   |
| `workspace-manager`は宣言的配置とAgent起動を実装しているが、その後のペイン内プロセスのライフサイクルは管理しない。[S4](sources.md#s4) | 配置機能と継続的なプロセス管理を分離して実装する。                                  |
| `serde_yaml`は保守終了を明記している。[S5](sources.md#s5)                                                                             | 前回の候補を変更し、YAML実装には`serde-saphyr`を採用候補とする。[S6](sources.md#s6) |
| Tokioの`Child`は、破棄しただけでは既定で子プロセスを停止しない。[S7](sources.md#s7)                                                   | 停止要求、猶予期間、強制終了、終了回収を明示的に実装する。                          |

Herdrの対応下限はまだ決めない。使用するAPIの存在と返却形式を検証し、その結果を`min_herdr_version`へ反映する。インストール済みバイナリのAPIスキーマ取得機能を互換性試験に利用する。[S1](sources.md#s1)

<a id="source-27"></a>

## 参考資料

公開資料の確認日は2026-09-11。ここに挙げた仕様を利用者のインストール済みバージョンで確認したわけではない。実装時は参照する版を固定し、実機試験の結果を対応表へ追加する。

| ID                  | 資料                                 | 参照先                                                           |
| ------------------- | ------------------------------------ | ---------------------------------------------------------------- |
| <a id="s1"></a>S1   | Herdr Socket API                     | `https://herdr.dev/docs/socket-api/`                             |
| <a id="s2"></a>S2   | Herdr Plugins                        | `https://herdr.dev/docs/plugins/`                                |
| <a id="s3"></a>S3   | Herdr Worktree Bootstrap・作者README | `https://github.com/zerodice0/herdr-plugin-worktree-bootstrap`   |
| <a id="s4"></a>S4   | Herdr Workspace Manager・作者README  | `https://github.com/razajamil/herdr-plugin-workspace-manager`    |
| <a id="s5"></a>S5   | serde_yaml・保守状況                 | `https://docs.rs/serde_yaml/latest/serde_yaml/`                  |
| <a id="s6"></a>S6   | serde-saphyr・API資料                | `https://docs.rs/serde-saphyr/latest/serde_saphyr/`              |
| <a id="s7"></a>S7   | Tokio process・終了処理の注意事項    | `https://docs.rs/tokio/latest/tokio/process/`                    |
| <a id="s8"></a>S8   | mise exec                            | `https://mise.jdx.dev/cli/exec.html`                             |
| <a id="s9"></a>S9   | Git worktree                         | `https://git-scm.com/docs/git-worktree`                          |
| <a id="s10"></a>S10 | Git hooks                            | `https://git-scm.com/docs/githooks`                              |
| <a id="s11"></a>S11 | mise trust                           | `https://mise.jdx.dev/cli/trust.html`                            |
| <a id="s12"></a>S12 | petgraph toposort                    | `https://docs.rs/petgraph/latest/petgraph/algo/fn.toposort.html` |
| <a id="s13"></a>S13 | rusqlite                             | `https://docs.rs/rusqlite/latest/rusqlite/`                      |
| <a id="s14"></a>S14 | Herdr Windows beta                   | `https://herdr.dev/docs/windows-beta/`                           |

## 文書構成の参考資料

| ID                | 資料                                               | 文書体系への適用                                                       | 参照先                                                                                                                                               |
| ----------------- | -------------------------------------------------- | ---------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------- |
| <a id="d1"></a>D1 | Diátaxis                                           | 動作規則、背景、操作手順を分ける。未実装製品のチュートリアルは作らない | `https://www.diataxis.fr/start-here/`                                                                                                                |
| <a id="d2"></a>D2 | Michael Nygard, Documenting Architecture Decisions | 判断ごとに背景、選択、結果、状態を記録し、置き換えた判断も残す         | `https://cognitect.com/blog/2011/11/15/documenting-architecture-decisions`                                                                           |
| <a id="d3"></a>D3 | GitHub Docs, Basic writing and formatting syntax   | リポジトリ内の相対リンクと明示アンカーを利用する                       | `https://docs.github.com/en/get-started/writing-on-github/getting-started-with-writing-and-formatting-on-github/basic-writing-and-formatting-syntax` |

## 外部仕様の再確認記録

実装開始時は、資料ID、対象releaseまたはcommit、実行環境、確認日、確認内容、試験結果を記録する。`latest`ページのURLだけを対応済み版の根拠にしない。未確認部分は未決事項へ移し、推測で埋めない。

## 関連文書

[互換性](../integrations/herdr.md) / [更新ルール](../maintenance.md)
