//! gridから二分割木への変換と検証(docs/specs/layout.md、T19)。
//! 端末サイズに応じた実サイズ計算・最低寸法チェックは実行時の話であり対象外。

use serde::Serialize;

use crate::config::model::{LayoutConfig, PaneId, TabConfig, TabId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SplitDirection {
    /// 左右分割。
    Right,
    /// 上下分割。
    Down,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum SplitNode {
    Leaf {
        pane: PaneId,
    },
    Split {
        direction: SplitDirection,
        first: Box<SplitNode>,
        second: Box<SplitNode>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SplitTree {
    pub root: SplitNode,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum LayoutError {
    #[error("pane {pane:?} in tab {tab:?} is out of the {columns}x{rows} grid")]
    OutOfRange {
        tab: TabId,
        pane: PaneId,
        columns: u32,
        rows: u32,
    },
    #[error("panes {first:?} and {second:?} in tab {tab:?} overlap")]
    Overlap {
        tab: TabId,
        first: PaneId,
        second: PaneId,
    },
    #[error("tab {tab:?} has an unfilled cell at column {column}, row {row}")]
    UnfilledCell { tab: TabId, column: u32, row: u32 },
    #[error("tab {tab:?} layout_not_sliceable: no row or column cut avoids crossing a pane")]
    NotSliceable { tab: TabId },
}

#[derive(Debug, Clone, Copy)]
struct ResolvedPane<'a> {
    id: &'a PaneId,
    col_start: u32,
    col_end: u32,
    row_start: u32,
    row_end: u32,
}

impl ResolvedPane<'_> {
    fn exactly_fills(&self, region: &Region) -> bool {
        self.col_start == region.col_start
            && self.col_end == region.col_end
            && self.row_start == region.row_start
            && self.row_end == region.row_end
    }

    fn area(&self) -> u64 {
        u64::from(self.col_end - self.col_start + 1) * u64::from(self.row_end - self.row_start + 1)
    }
}

#[derive(Debug, Clone, Copy)]
struct Region {
    col_start: u32,
    col_end: u32,
    row_start: u32,
    row_end: u32,
}

fn resolve_panes(tab: &TabConfig) -> Vec<ResolvedPane<'_>> {
    tab.panes
        .iter()
        .map(|pane| ResolvedPane {
            id: &pane.id,
            col_start: pane.placement.column,
            col_end: pane.placement.column + pane.placement.col_span - 1,
            row_start: pane.placement.row,
            row_end: pane.placement.row + pane.placement.row_span - 1,
        })
        .collect()
}

fn rects_intersect(a: &ResolvedPane, b: &ResolvedPane) -> bool {
    a.col_start <= b.col_end
        && b.col_start <= a.col_end
        && a.row_start <= b.row_end
        && b.row_start <= a.row_end
}

fn first_uncovered_cell(panes: &[ResolvedPane], columns: u32, rows: u32) -> (u32, u32) {
    for row in 1..=rows {
        for column in 1..=columns {
            let covered = panes.iter().any(|p| {
                p.col_start <= column
                    && column <= p.col_end
                    && p.row_start <= row
                    && row <= p.row_end
            });
            if !covered {
                return (column, row);
            }
        }
    }
    // 面積合計で不整合が検出された時点で必ずどこかに未配置セルがあるはずだが、
    // 呼び出し側の不変条件が崩れた場合の保険として(1,1)を返す。
    (1, 1)
}

fn validate_grid(
    tab: &TabId,
    panes: &[ResolvedPane],
    columns: u32,
    rows: u32,
) -> Result<(), LayoutError> {
    for p in panes {
        if p.col_start < 1 || p.row_start < 1 || p.col_end > columns || p.row_end > rows {
            return Err(LayoutError::OutOfRange {
                tab: tab.clone(),
                pane: p.id.clone(),
                columns,
                rows,
            });
        }
    }
    for i in 0..panes.len() {
        for j in (i + 1)..panes.len() {
            if rects_intersect(&panes[i], &panes[j]) {
                return Err(LayoutError::Overlap {
                    tab: tab.clone(),
                    first: panes[i].id.clone(),
                    second: panes[j].id.clone(),
                });
            }
        }
    }
    let total_area: u64 = panes.iter().map(ResolvedPane::area).sum();
    if total_area != u64::from(columns) * u64::from(rows) {
        let (column, row) = first_uncovered_cell(panes, columns, rows);
        return Err(LayoutError::UnfilledCell {
            tab: tab.clone(),
            column,
            row,
        });
    }
    Ok(())
}

fn no_pane_straddles_column(panes: &[ResolvedPane], region: &Region, col: u32) -> bool {
    panes
        .iter()
        .filter(|p| p.col_start >= region.col_start && p.col_end <= region.col_end)
        .filter(|p| p.row_start >= region.row_start && p.row_end <= region.row_end)
        .all(|p| !(p.col_start <= col && col < p.col_end))
}

fn no_pane_straddles_row(panes: &[ResolvedPane], region: &Region, row: u32) -> bool {
    panes
        .iter()
        .filter(|p| p.col_start >= region.col_start && p.col_end <= region.col_end)
        .filter(|p| p.row_start >= region.row_start && p.row_end <= region.row_end)
        .all(|p| !(p.row_start <= row && row < p.row_end))
}

fn split_region(
    tab: &TabId,
    region: &Region,
    panes: &[ResolvedPane],
) -> Result<SplitNode, LayoutError> {
    if let Some(pane) = panes.iter().find(|p| p.exactly_fills(region)) {
        return Ok(SplitNode::Leaf {
            pane: pane.id.clone(),
        });
    }

    for col in region.col_start..region.col_end {
        if no_pane_straddles_column(panes, region, col) {
            let left = Region {
                col_end: col,
                ..*region
            };
            let right = Region {
                col_start: col + 1,
                ..*region
            };
            return Ok(SplitNode::Split {
                direction: SplitDirection::Right,
                first: Box::new(split_region(tab, &left, panes)?),
                second: Box::new(split_region(tab, &right, panes)?),
            });
        }
    }

    for row in region.row_start..region.row_end {
        if no_pane_straddles_row(panes, region, row) {
            let top = Region {
                row_end: row,
                ..*region
            };
            let bottom = Region {
                row_start: row + 1,
                ..*region
            };
            return Ok(SplitNode::Split {
                direction: SplitDirection::Down,
                first: Box::new(split_region(tab, &top, panes)?),
                second: Box::new(split_region(tab, &bottom, panes)?),
            });
        }
    }

    Err(LayoutError::NotSliceable { tab: tab.clone() })
}

pub fn compile_layout(tab: &TabConfig) -> Result<SplitTree, LayoutError> {
    let LayoutConfig::Grid { columns, rows } = &tab.layout;
    let panes = resolve_panes(tab);
    validate_grid(&tab.id, &panes, *columns, *rows)?;
    let region = Region {
        col_start: 1,
        col_end: *columns,
        row_start: 1,
        row_end: *rows,
    };
    let root = split_region(&tab.id, &region, &panes)?;
    Ok(SplitTree { root })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::model::{PaneConfig, PaneView, Placement};

    fn pane(id: &str, column: u32, row: u32, col_span: u32, row_span: u32) -> PaneConfig {
        PaneConfig {
            id: PaneId(id.to_string()),
            label: id.to_string(),
            view: PaneView::Shell,
            placement: Placement {
                column,
                row,
                col_span,
                row_span,
            },
        }
    }

    fn tab_with(columns: u32, rows: u32, panes: Vec<PaneConfig>) -> TabConfig {
        TabConfig {
            id: TabId::from("t"),
            label: "t".to_string(),
            layout: LayoutConfig::Grid { columns, rows },
            panes,
        }
    }

    fn example_tab() -> TabConfig {
        let yaml = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/workflow.yaml"),
        )
        .unwrap();
        let spec: crate::config::model::WorkflowSpec = serde_yaml_ng::from_str(&yaml).unwrap();
        spec.workspace.tabs[0].clone()
    }

    #[test]
    fn example_workflow_grid_is_sliceable() {
        let tree = compile_layout(&example_tab()).expect("example grid should be sliceable");
        let expected = SplitTree {
            root: SplitNode::Split {
                direction: SplitDirection::Down,
                first: Box::new(SplitNode::Split {
                    direction: SplitDirection::Right,
                    first: Box::new(SplitNode::Leaf {
                        pane: PaneId("A".to_string()),
                    }),
                    second: Box::new(SplitNode::Leaf {
                        pane: PaneId("B".to_string()),
                    }),
                }),
                second: Box::new(SplitNode::Split {
                    direction: SplitDirection::Right,
                    first: Box::new(SplitNode::Leaf {
                        pane: PaneId("C".to_string()),
                    }),
                    second: Box::new(SplitNode::Split {
                        direction: SplitDirection::Right,
                        first: Box::new(SplitNode::Leaf {
                            pane: PaneId("D".to_string()),
                        }),
                        second: Box::new(SplitNode::Leaf {
                            pane: PaneId("E".to_string()),
                        }),
                    }),
                }),
            },
        };
        assert_eq!(tree, expected);
    }

    #[test]
    fn placement_out_of_column_range_is_rejected() {
        let tab = tab_with(2, 1, vec![pane("A", 1, 1, 3, 1)]);
        let err = compile_layout(&tab).unwrap_err();
        assert!(matches!(err, LayoutError::OutOfRange { .. }));
    }

    #[test]
    fn placement_out_of_row_range_is_rejected() {
        let tab = tab_with(1, 1, vec![pane("A", 1, 1, 1, 2)]);
        let err = compile_layout(&tab).unwrap_err();
        assert!(matches!(err, LayoutError::OutOfRange { .. }));
    }

    #[test]
    fn overlapping_placements_are_rejected() {
        let tab = tab_with(2, 1, vec![pane("A", 1, 1, 2, 1), pane("B", 2, 1, 1, 1)]);
        let err = compile_layout(&tab).unwrap_err();
        assert!(matches!(err, LayoutError::Overlap { .. }));
    }

    #[test]
    fn unfilled_cell_is_rejected() {
        let tab = tab_with(2, 1, vec![pane("A", 1, 1, 1, 1)]);
        let err = compile_layout(&tab).unwrap_err();
        assert!(matches!(err, LayoutError::UnfilledCell { .. }));
    }

    #[test]
    fn pinwheel_layout_is_not_sliceable() {
        // P1: col1-2,row1 / P2: col3,row1-2 / P3: col2-3,row3 / P4: col1,row2-3 / P5: col2,row2
        let tab = tab_with(
            3,
            3,
            vec![
                pane("P1", 1, 1, 2, 1),
                pane("P2", 3, 1, 1, 2),
                pane("P3", 2, 3, 2, 1),
                pane("P4", 1, 2, 1, 2),
                pane("P5", 2, 2, 1, 1),
            ],
        );
        let err = compile_layout(&tab).unwrap_err();
        assert_eq!(
            err,
            LayoutError::NotSliceable {
                tab: TabId::from("t")
            }
        );
    }
}
