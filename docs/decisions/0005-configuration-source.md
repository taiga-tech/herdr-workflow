---
id: ADR-0005
title: 'ADR-0005：設定定義と生成スキーマの管理'
status: proposed
documentVersion: '0.2'
updated: '2026-09-14'
---

# ADR-0005：設定定義と生成スキーマの管理

[文書目次](../README.md) · [更新ルール](../maintenance.md)

この文書の管理対象：一つの判断の背景、選択と結果。現在の動作規則は関連仕様を参照する。

## 背景

YAML、Rust型、JSON Schema、文書中の完全な例を別々に更新すると、型や参照先の説明が一致しなくなる。現行の設定例には全機能の入力形式がまだ定義されていない。

## 提案する決定

実装後はRustの設定型から構造スキーマを生成し、DAGやgrid等は意味検証で扱う。現在の設定例は`examples/workflow.yaml`を編集元とし、説明文書から参照する。

## 比較した案

JSON Schemaを手作業で並行管理する案、設定例を各ガイドへ全文転載する案は、同じ規則の編集箇所が増えるため採らない案とする。

## 結果と制約

Rust型と生成スキーマが存在しない状態では、文書検査の合格を設定スキーマへの適合と呼ばない。

## 採用条件

未知キーと重複キー、値置換、参照、DAG、gridの検証を実装し、Q11の未定義キーを決める。YAMLライブラリの版は依存関係の確認を経て選定する。

## YAMLライブラリの選定

`serde_yaml`は開発が停止しアーカイブされているため、そのフォークである`serde_yaml_ng`をCargo.tomlの依存として採用した(`src/config/load.rs`)。未知キーの拒否は`WorkflowSpec`側の`deny_unknown_fields`で実装済みだが、重複キー検出、値置換、DAGの意味検証の一部、gridの検証は未実装のため、他の採用条件は引き続き未解決として扱い、この文書の状態は`proposed`のまま維持する。

## JSON Schema生成の実装

未知キー・重複キー検出、値置換、DAG(循環依存・依存条件)、gridの検証をすべて実装した後、`schemars`(Cargo.tomlの依存)で`WorkflowSpec`からJSON Schemaを生成する`herdr-workflow schema`コマンドを実装した(`src/cli.rs`)。生成物は`schema/workflow.schema.json`としてリポジトリに追跡する。`jsonschema`(dev依存)で`examples/workflow.yaml`が生成スキーマに対して妥当であることと、JSON Schemaが構造検証のみを担いDAGの意味検証を代替しないことを`tests/schema_contract.rs`で確認した。

採用条件のうち「Q11の未定義キーを決める」は本実装と無関係に未解決のまま残っているため、この文書の状態は引き続き`proposed`とする。

## 関連文書

[設定](../reference/configuration.md) / [未決事項](../planning/open-questions.md) / [試験](../testing/acceptance.md)
