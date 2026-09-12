use std::collections::{BTreeMap, BTreeSet};

use crate::config::model::TaskId;

/// `depends_on[task]`は`task`が依存する先(先に完了すべきTaskId)の一覧を表す。
/// 依存先がすべて先に来る順序を返す。同点はTask ID昇順とし、同じ計画で
/// 起動順の説明が変わらないようにする(docs/specs/workflow.md「実行可能タスクの
/// 選択はTask ID順を既定とする」)。循環がある場合は循環経路を`Err`で返す。
pub fn topo_order(
    all_tasks: &BTreeSet<TaskId>,
    depends_on: &BTreeMap<TaskId, Vec<TaskId>>,
) -> Result<Vec<TaskId>, Vec<TaskId>> {
    let empty: Vec<TaskId> = Vec::new();
    let mut in_degree: BTreeMap<&TaskId, usize> = all_tasks
        .iter()
        .map(|task| (task, depends_on.get(task).unwrap_or(&empty).len()))
        .collect();

    let mut dependents: BTreeMap<&TaskId, Vec<&TaskId>> =
        all_tasks.iter().map(|task| (task, Vec::new())).collect();
    for task in all_tasks {
        for dependency in depends_on.get(task).unwrap_or(&empty) {
            dependents.entry(dependency).or_default().push(task);
        }
    }

    let mut ready: BTreeSet<&TaskId> = in_degree
        .iter()
        .filter(|(_, degree)| **degree == 0)
        .map(|(task, _)| *task)
        .collect();

    let mut order = Vec::with_capacity(all_tasks.len());
    while let Some(task) = ready.iter().next().copied() {
        ready.remove(task);
        order.push(task.clone());
        for dependent in &dependents[task] {
            let degree = in_degree.get_mut(dependent).expect("dependent is tracked");
            *degree -= 1;
            if *degree == 0 {
                ready.insert(dependent);
            }
        }
    }

    if order.len() == all_tasks.len() {
        Ok(order)
    } else {
        let remaining: BTreeSet<&TaskId> = all_tasks
            .iter()
            .filter(|task| !order.contains(task))
            .collect();
        Err(find_cycle(&remaining, depends_on))
    }
}

/// `remaining`(トポロジカルソートで解決できなかったタスク集合)の中からDFSで
/// 戻り辺を検出し、実際の循環経路を再構築する。
fn find_cycle(
    remaining: &BTreeSet<&TaskId>,
    depends_on: &BTreeMap<TaskId, Vec<TaskId>>,
) -> Vec<TaskId> {
    let empty: Vec<TaskId> = Vec::new();
    let mut visiting: Vec<TaskId> = Vec::new();
    let mut visited: BTreeSet<TaskId> = BTreeSet::new();

    fn visit(
        task: &TaskId,
        remaining: &BTreeSet<&TaskId>,
        depends_on: &BTreeMap<TaskId, Vec<TaskId>>,
        empty: &Vec<TaskId>,
        visiting: &mut Vec<TaskId>,
        visited: &mut BTreeSet<TaskId>,
    ) -> Option<Vec<TaskId>> {
        if let Some(start) = visiting.iter().position(|t| t == task) {
            let mut cycle = visiting[start..].to_vec();
            cycle.push(task.clone());
            return Some(cycle);
        }
        if visited.contains(task) {
            return None;
        }
        visiting.push(task.clone());
        for dependency in depends_on.get(task).unwrap_or(empty) {
            if remaining.contains(dependency)
                && let Some(cycle) =
                    visit(dependency, remaining, depends_on, empty, visiting, visited)
            {
                return Some(cycle);
            }
        }
        visiting.pop();
        visited.insert(task.clone());
        None
    }

    for task in remaining {
        if let Some(cycle) = visit(
            task,
            remaining,
            depends_on,
            &empty,
            &mut visiting,
            &mut visited,
        ) {
            return cycle;
        }
    }
    remaining.iter().map(|t| (*t).clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tasks(ids: &[&str]) -> BTreeSet<TaskId> {
        ids.iter().map(|id| TaskId::from(*id)).collect()
    }

    fn deps(pairs: &[(&str, &[&str])]) -> BTreeMap<TaskId, Vec<TaskId>> {
        pairs
            .iter()
            .map(|(task, on)| {
                (
                    TaskId::from(*task),
                    on.iter().map(|id| TaskId::from(*id)).collect(),
                )
            })
            .collect()
    }

    #[test]
    fn linear_chain_orders_dependencies_first() {
        let all = tasks(&["a", "b", "c"]);
        let depends_on = deps(&[("b", &["a"]), ("c", &["b"])]);
        let order = topo_order(&all, &depends_on).expect("should not cycle");
        assert_eq!(
            order,
            vec!["a", "b", "c"]
                .into_iter()
                .map(TaskId::from)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn diamond_dependency_orders_all_prerequisites_first() {
        let all = tasks(&["a", "b", "c", "d"]);
        let depends_on = deps(&[("b", &["a"]), ("c", &["a"]), ("d", &["b", "c"])]);
        let order = topo_order(&all, &depends_on).expect("should not cycle");
        let index = |id: &str| order.iter().position(|t| t.0 == id).unwrap();
        assert!(index("a") < index("b"));
        assert!(index("a") < index("c"));
        assert!(index("b") < index("d"));
        assert!(index("c") < index("d"));
    }

    #[test]
    fn disconnected_components_are_ordered_by_task_id() {
        let all = tasks(&["z", "a"]);
        let depends_on = deps(&[]);
        let order = topo_order(&all, &depends_on).expect("should not cycle");
        assert_eq!(
            order,
            vec!["a", "z"]
                .into_iter()
                .map(TaskId::from)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn self_cycle_is_detected() {
        let all = tasks(&["a"]);
        let depends_on = deps(&[("a", &["a"])]);
        let cycle = topo_order(&all, &depends_on).expect_err("self cycle should be rejected");
        assert_eq!(cycle, vec![TaskId::from("a"), TaskId::from("a")]);
    }

    #[test]
    fn three_node_cycle_reports_the_cycle_path() {
        let all = tasks(&["a", "b", "c"]);
        let depends_on = deps(&[("a", &["b"]), ("b", &["c"]), ("c", &["a"])]);
        let cycle = topo_order(&all, &depends_on).expect_err("cycle should be rejected");
        assert_eq!(cycle.first(), cycle.last());
        assert_eq!(cycle.len(), 4);
        let ids: BTreeSet<&str> = cycle.iter().map(|t| t.0.as_str()).collect();
        assert_eq!(ids, ["a", "b", "c"].into_iter().collect());
    }
}
