---
id: DOC-ROOT
title: "Herdr Workflow 設計資料"
status: current
documentVersion: "0.2"
updated: "2026-09-11"
---

# Herdr Workflow 設計資料

[文書目次](docs/README.md) · [更新ルール](docs/maintenance.md)

## 文書の入口

[文書目次](docs/README.md)から参照する。主要な文書は[アーキテクチャ](docs/architecture.md)、[機能一覧](docs/features.md)、[ディレクトリ構成](docs/repository-structure.md)。

これはHerdr用ワークフロープラグインの設計資料一式であり、プラグインの実装や実行可能なRustプロジェクトではない。Rust採用以外の設計判断は、文書とADRに記載した状態に従う。

## 収録内容

元の27章と付録を、仕様、リファレンス、連携、手順、試験、計画、設計判断に分けた。T01〜T32を維持し、機能IDから仕様と試験へ参照できるようにした。

設定例の編集元は[examples/workflow.yaml](examples/workflow.yaml)。元の設計書と設定例は[archive/](archive/README.md)に保存し、[移行表](docs/meta/source-map.md)から旧章をたどれる。

## リポジトリへの導入

ZIPを展開し、文書と設定例を実装用リポジトリで版管理する。既存の`README.md`、`AGENTS.md`、`docs/`がある場合は上書きせず、内容を照合して取り込む。既存READMEには文書目次へのリンクを追加する。

この配布物の作成ではGitHub等へのコミット、既存リポジトリの変更、公開は行っていない。どこへ保存した場合も、この相対パス構造を保って参照する。

## 文書検査

```bash
python3 scripts/check_docs.py
```

Python標準ライブラリだけで動作する。[今回の検査記録](docs/meta/validation-report.md)に検査範囲を記載した。製品の動作、Rustのコンパイル、Herdr APIへの接続はこの検査の対象外。

更新時は[文書更新ルール](docs/maintenance.md)と[変更履歴](CHANGELOG.md)を使う。AIによる作業では[AGENTS.md](AGENTS.md)を入口にする。

旧名で参照する場合は[herdr-workflow-design.md](herdr-workflow-design.md)から移行先を確認できる。
