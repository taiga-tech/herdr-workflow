---
id: ADR-0007
title: 'ADR-0007：状態ディレクトリ、制御通信、プロセス識別'
status: accepted
documentVersion: '0.2'
updated: '2026-11-05'
---

# ADR-0007：状態ディレクトリ、制御通信、プロセス識別

[文書目次](../README.md) · [更新ルール](../maintenance.md)

この文書の管理対象：一つの判断の背景、選択と結果。現在の動作規則は[状態ストア](../specs/persistence.md)を参照する。

## 背景

段階Bの非対話実行では、状態DB、Attemptの結果ファイルとログ、Coordinatorの制御ソケットを置く場所と、CLI・hookからCoordinatorへの要求形式、再起動後に子プロセスを同一と判定する方法が必要になる。これらは未決事項[Q12](../planning/open-questions.md#q12)として保留していた。

Herdr v0.9.0は、プラグインの実行時コマンドへ`HERDR_PLUGIN_STATE_DIR`(XDG状態ディレクトリ配下のプラグインID別ディレクトリ)を渡す。[S2](../reference/sources.md#s2) 一方、本プラグインのCLIはHerdrの外からも実行され、plugin IDは公開前に変わる可能性がある([Q01](../planning/open-questions.md#q01))。

## 決定

1. 状態ディレクトリはHerdrのプラグイン状態ディレクトリから独立した、Herdr Workflow専用のディレクトリとする。Herdr内外のどちらから起動しても同じ規則で同じパスを決める。
2. CLI・hookとCoordinatorの制御通信は、状態ディレクトリ内のUnix domain socketで、版番号付きのJSON Lines(1行に1つのJSONオブジェクト)による要求・応答とする。Windows named pipeへの差し替えは段階Eで扱う。
3. 子プロセスは、PID、OSが報告するプロセス起動時刻、プロセスグループID、ブートの識別子の組で識別し、すべて一致した場合だけ同一プロセスとみなす。

パス、権限、メッセージ形式、照合の判定表は[状態ストア](../specs/persistence.md)で管理する。

## 比較した案

- `HERDR_PLUGIN_STATE_DIR`を優先し、Herdr外では同じ規則でパスを計算する案：Herdrの内部パス規則とplugin IDに状態の置き場所が依存し、ID変更やHerdr側の規則変更でDBが分断されるため採らない。
- `HERDR_PLUGIN_STATE_DIR`がない場合に明示指定を必須とする案：Herdr外のCLI利用で毎回指定が必要になり、指定漏れによる別DBの生成を防げないため採らない。
- 受付ディレクトリへ要求ファイルを置くスプール方式：同期応答が遅く、受付完了の確認をポーリングに頼るため採らない。
- 長さ前置のバイナリフレーム：大きな応答には強いが、手動診断がしにくく初期版の要求規模では利点が小さいため採らない。
- PIDと起動時刻だけ、またはRunnerのソケット生存だけで判定する案：前者は子孫プロセスの停止対象を決められず、後者はRunnerの異常終了時に子プロセスの状態を判定できないため採らない。

## 結果と制約

Herdrの`plugin uninstall`ではこの状態ディレクトリを削除しない。状態の削除は本プラグインの後片付け手順で扱う。

ソケットパスがOSの長さ上限を超える場合は、一時ディレクトリへ移さずエラーにする。起動時刻やプロセスグループを取得できないOSでは、同一性を`unknown`として扱い、プロセスを自動で再起動・停止しない。

この判断はCoordinatorとTaskRunnerの分離([ADR-0002](0002-coordinator-runner.md))の採否とは独立に有効である。ADR-0002を採らない場合も、状態ディレクトリ、制御通信、プロセス識別の規則は維持する。

## 確認記録

`HERDR_PLUGIN_STATE_DIR`の存在と、Herdrが同ディレクトリを`$XDG_STATE_HOME/herdr/plugins/<plugin-id>`(未設定時は`$HOME/.local/state/herdr/...`)に置くことは、Herdr v0.9.0タグのソース(`src/app/api/plugins/env.rs`、`src/plugin_paths.rs`、`src/config/io.rs`)とplugins.mdxで確認した。実機での受け渡しは未試験である。

## 関連文書

[状態ストア](../specs/persistence.md) / [復旧](../specs/recovery.md) / [承認と権限](../specs/security.md) / [未決事項](../planning/open-questions.md)
