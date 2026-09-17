---
id: SPEC-LAYOUT
title: 'gridとペイン配置'
status: draft
documentVersion: '0.2'
updated: '2026-09-17'
---

# gridとペイン配置

[文書目次](../README.md) · [更新ルール](../maintenance.md)

> statusは`draft`です。実装済みとして扱う範囲は、コードと試験結果で確認します。

この文書の管理対象：座標、検証、分割木への変換、再適用。

<a id="source-08"></a>

## gridとペイン配置

### 座標と範囲

`column`と`row`は1始まり、`colSpan`と`rowSpan`は正の整数とする。gridの`columns`と`rows`はそれぞれ1以上、プラグイン全体設定`limits.layout.maxGridDimension`以下とし、一つのtabのpane数は`limits.layout.maxPanesPerTab`以下とする。省略時および設定可能な安全上限はgrid各軸が64、pane数が32で、設定ではこれらを引き下げられるが引き上げられない。列幅と行高は等分を初期仕様とし、比率指定は後続の拡張に分ける。

[設定例](../../examples/workflow.yaml)の配置は次のセル割り当てになる。

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

検証ではgridの寸法上限超過、tabのpane数上限超過、範囲外、重なり、未配置セル、参照切れを拒否する。pane数上限は二次時間となる重複検査より先に適用する。空き領域を残す場合は、明示的にshell用のペインを配置する。

### 分割木への変換

Herdr側のレイアウトは二分割木で表現されるため、gridを内部の分割木へ変換する。[S1](../reference/sources.md#s1)

変換器は、ペインを横切らずに領域を二分できる行境界または列境界を探し、両側へ再帰する。候補の探索順を固定する。左右分割は`right`、上下分割は`down`に変換する。各分割ノードには、分割対象領域の列数または行数を`total_span`、先頭側(左または上)の列数または行数を`first_span`として整数で保存する。JSONではそれぞれ`totalSpan`、`firstSpan`として出力する。子領域の比率は`first_span : (total_span - first_span)`で復元できる。これらは各再帰領域を基準にしたgridセル数であり、端末の実サイズではない。

任意の矩形配置を二分割木へ変換できるとは扱わない。分割できない配置は`layout_not_sliceable`として事前に拒否し、見た目を近似した配置へ黙って変更しない。

端末の列数・行数と罫線幅を踏まえて実際の大きさを計算する。端数処理を固定し、各ペインの最低寸法を下回る場合は配置を開始しない。縮小された端末への対応では、実行中タスクを作り直さず表示上の制約として扱う。

### 再適用

作成直後は新しいタブを追加する。Herdrがworktree作成時に用意した最初のタブは、既定で残す。

`resume`は既存タブへ`layout.apply`を再実行しない。所有するペインIDと現在の構成を照合し、欠損や利用者の変更を検出した場合は差分を提示する。配置だけを変える処理と、タブを置換する処理は別コマンドにする。

## 関連文書

[workflow設定例](../../examples/workflow.yaml) / [プラグイン全体設定](../reference/configuration.md) / [Herdr連携](../integrations/herdr.md) / [再開](recovery.md)
