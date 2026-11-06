---
id: ADR-0008
title: 'ADR-0008：対話コマンドの擬似端末をTaskRunnerが持つ'
status: accepted
documentVersion: '0.2'
updated: '2026-11-06'
---

# ADR-0008：対話コマンドの擬似端末をTaskRunnerが持つ

[文書目次](../README.md) · [更新ルール](../maintenance.md)

この文書の管理対象：一つの判断の背景、選択と結果。現在の動作規則は[実行](../specs/execution.md#interactive)を参照する。

## 背景

入力を求めるコマンド(ログイン、確認付きのmigration等)を管理対象のTaskとして実行するには、キー入力、前景プロセスグループ、Ctrl+C、端末サイズの変更を子プロセスへ正しく届ける必要がある([Q04](../planning/open-questions.md#q04))。当初の案は`runner: pane`として、HerdrのpaneのPTY内で専用ラッパーを動かし、ラッパーから起動と終了を報告するものだった。

この案では、paneを閉じるとPTYが閉じて子プロセスが終了し、ログ用paneを閉じてもプロセスを止めない非対話コマンドの規則([実行](../specs/execution.md))と食い違う。Coordinatorが再起動したときの照合も、Herdrのpaneの寿命に依存する。Herdrがない環境(単独CLI)や試験環境では実行できない。

## 決定

1. 対話コマンドは`runner: pty`とする。TaskRunner([ADR-0002](0002-coordinator-runner.md))が擬似端末を確保し、子プロセスを新しいセッションで起動して、その擬似端末を制御端末にする。
2. 利用者の端末は`herdr-workflow term`で接続する。接続はAttemptごとのUnix domain socket(`0600`、接続元の利用者IDを確認)で行う。クライアントは自分の端末をrawモードにしてキー入力をそのまま送り、端末サイズを送る。TaskRunnerは出力をそのまま返す。
3. Ctrl+Cは擬似端末の規則によって前景プロセスグループへSIGINTとして届く。切り離しはCtrl+]で行い、子プロセスは止めない。
4. Herdr連携では、`runner: pty`のタスクを表示する`logs`のpaneが`term --keep`を実行する。
5. 擬似端末の出力は標準出力のログとして記録する。標準出力と標準エラーは区別しない。

## 比較した案

- `runner: pane`(paneのPTY内でラッパーを動かす)：paneを閉じると子プロセスが終了し、Coordinatorの再起動時の照合がpaneの寿命に依存し、Herdrなしでは実行も試験もできないため採らない。
- 端末の多重化ツール(tmux、dtach等)を外部コマンドとして使う：利用者環境への追加依存と、ツールごとの終了・信号の扱いの差が生じるため採らない。
- 標準入力だけをパイプで渡す：前景プロセスグループ、Ctrl+C、端末サイズの変更を再現できず、端末を前提とするコマンドが動かないため採らない。

## 結果と制約

接続していない間も子プロセスは動き続ける。入力待ちで止まった場合は、jobの実行期限とログで利用者へ返す。接続時には直近64 KiBの出力を送り直す。複数の端末から同時に接続でき、入力はどの端末からも受け付ける。

ソケットはAttemptのディレクトリに置き、パスがOSの長さ上限を超える場合は状態ディレクトリ直下の`tty/`に短い名前で置く。それでも超える場合は起動失敗にする。Windowsネイティブ(ConPTY)は段階Eの別判断とする。

## 確認記録

Linux上の実プロセスの試験(`src/runtime/runner.rs`、`tests/cli_runtime.rs`)で、キー入力の中継、Ctrl+Cによる前景プロセスグループへのSIGINT、端末サイズの変更によるSIGWINCH、切り離し後の再接続と直近の出力の再送、子プロセスから見た標準入力が端末であることを確認した([T40](../testing/acceptance.md#t40)〜[T43](../testing/acceptance.md#t43))。macOSとHerdr実機のpaneでの動作は未試験である。この判断は、利用者が判断待ちの事項をこちらで決めるよう委ねたことに基づく。

## 関連文書

[実行](../specs/execution.md) / [CLI](../reference/cli.md) / [ADR-0002](0002-coordinator-runner.md) / [未決事項](../planning/open-questions.md)
