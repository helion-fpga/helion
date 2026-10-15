//! Parallel module elaboration.
//!
//! Elaborates independent modules in parallel using rayon.
//! Modules are topologically sorted by dependency (instantiation) level.
//! Each level is elaborated in parallel; levels are processed sequentially.
//!
//! This avoids the need for thread-safe Design by giving each thread its
//! own Design and merging sequentially after each level.

use std::collections::{HashMap, HashSet, VecDeque};

/// Build dependency graph: module -> set of modules it instantiates.
pub fn build_dep_graph(
    modules: &HashMap<String, Vec<String>>,
) -> HashMap<String, HashSet<String>> {
    let mut graph = HashMap::new();
    for (name, insts) in modules {
        graph.insert(name.clone(), insts.iter().cloned().collect());
    }
    graph
}

/// Topological sort into levels using Kahn's algorithm.
/// Returns Vec of levels; each level is a Vec of module names.
/// Modules in the same level have no dependencies on each other and
/// can be elaborated in parallel.
pub fn topo_sort_levels(
    graph: &HashMap<String, HashSet<String>>,
) -> Vec<Vec<String>> {
    // Compute in-degree (number of dependencies)
    let mut in_degree: HashMap<String, usize> = HashMap::new();
    let mut dependents: HashMap<String, Vec<String>> = HashMap::new();

    for (node, deps) in graph {
        in_degree.entry(node.clone()).or_insert(0);
        for dep in deps {
            // node depends on dep; dep must come first
            *in_degree.entry(node.clone()).or_insert(0) += 1;
            dependents
                .entry(dep.clone())
                .or_insert_with(Vec::new)
                .push(node.clone());
            in_degree.entry(dep.clone()).or_insert(0);
        }
    }

    // Kahn's algorithm, level by level
    let mut levels = Vec::new();
    let mut queue: VecDeque<String> = in_degree
        .iter()
        .filter(|(_n, deg)| **deg == 0)
        .map(|(n, _)| n.clone())
        .collect();

    while !queue.is_empty() {
        let level_size = queue.len();
        let mut level = Vec::with_capacity(level_size);

        for _ in 0..level_size {
            let node = queue.pop_front().unwrap();
            level.push(node.clone());

            if let Some(deps) = dependents.get(&node) {
                for dependent in deps {
                    let deg = in_degree.get_mut(dependent).unwrap();
                    *deg -= 1;
                    if *deg == 0 {
                        queue.push_back(dependent.clone());
                    }
                }
            }
        }

        levels.push(level);
    }

    levels
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topo_sort_linear() {
        // A -> B -> C (A instantiates B, B instantiates C)
        let mut graph = HashMap::new();
        graph.insert("A".to_string(), ["B".to_string()].into_iter().collect());
        graph.insert("B".to_string(), ["C".to_string()].into_iter().collect());
        graph.insert("C".to_string(), HashSet::new());

        let levels = topo_sort_levels(&graph);
        assert_eq!(levels.len(), 3);
        assert_eq!(levels[0], vec!["C".to_string()]);
        assert_eq!(levels[1], vec!["B".to_string()]);
        assert_eq!(levels[2], vec!["A".to_string()]);
    }

    #[test]
    fn topo_sort_parallel() {
        // Top instantiates A and B (independent)
        let mut graph = HashMap::new();
        graph.insert(
            "Top".to_string(),
            ["A".to_string(), "B".to_string()]
                .into_iter()
                .collect(),
        );
        graph.insert("A".to_string(), HashSet::new());
        graph.insert("B".to_string(), HashSet::new());

        let levels = topo_sort_levels(&graph);
        assert_eq!(levels.len(), 2);
        // Level 0: A and B (order may vary)
        assert_eq!(levels[0].len(), 2);
        assert!(levels[0].contains(&"A".to_string()));
        assert!(levels[0].contains(&"B".to_string()));
        // Level 1: Top
        assert_eq!(levels[1], vec!["Top".to_string()]);
    }

    #[test]
    fn topo_sort_empty() {
        let graph: HashMap<String, HashSet<String>> = HashMap::new();
        let levels = topo_sort_levels(&graph);
        assert!(levels.is_empty());
    }
}
