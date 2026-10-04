//! Bounded simple-cycle enumeration for the ADP check
//! (`find_adp_violations`), shared by the native tools and the Python
//! analysis through `_core`.
//!
//! Every cycle is found once, from its smallest node by name, walking
//! successors in name order; the walk stops after `max_cycles` kept cycles
//! (or `max_steps` steps), so a truncated answer is the same prefix on every
//! run. `networkx.simple_cycles` visited nodes in hash order, so its first
//! 5000 cycles changed from one process to the next.

/// What [`bounded_simple_cycles`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundedCycles {
    /// Kept cycles as node indices into the caller's list, each starting at
    /// its smallest node by name.
    pub cycles: Vec<Vec<usize>>,
    /// Cycles met, the one that hit the limit and short ones included.
    pub examined: usize,
    /// The walk stopped before meeting every cycle.
    pub truncated: bool,
}

/// Simple cycles of at most `max_length` nodes among `names`, through the
/// directed `edges` (duplicates ignored), keeping those of at least
/// `min_size` nodes and at most `max_cycles` of them.
pub fn bounded_simple_cycles(
    names: &[String],
    edges: &[(usize, usize)],
    min_size: usize,
    max_length: usize,
    max_cycles: usize,
    max_steps: usize,
) -> BoundedCycles {
    let count = names.len();
    // Rank by name: the walk's order, independent of the caller's.
    let mut by_name: Vec<usize> = (0..count).collect();
    by_name.sort_by(|left, right| names[*left].cmp(&names[*right]));
    let mut rank = vec![0; count];
    for (position, node) in by_name.iter().enumerate() {
        rank[*node] = position;
    }
    let mut successors: Vec<Vec<usize>> = vec![Vec::new(); count];
    for &(from, to) in edges {
        if from < count && to < count {
            successors[rank[from]].push(rank[to]);
        }
    }
    for list in &mut successors {
        list.sort_unstable();
        list.dedup();
    }

    let mut found = BoundedCycles {
        cycles: Vec::new(),
        examined: 0,
        truncated: false,
    };
    let mut steps = 0_usize;
    'starts: for start in 0..count {
        let mut path = vec![start];
        let mut on_path = vec![false; count];
        on_path[start] = true;
        let mut cursors: Vec<usize> = vec![0];
        while let Some(cursor) = cursors.last_mut() {
            steps += 1;
            if steps > max_steps {
                found.truncated = true;
                break 'starts;
            }
            let Some(&node) = path.last() else { break };
            let Some(&next) = successors[node].get(*cursor) else {
                cursors.pop();
                if let Some(left) = path.pop() {
                    on_path[left] = false;
                }
                continue;
            };
            *cursor += 1;
            if next == start {
                if path.len() > max_length {
                    continue;
                }
                found.examined += 1;
                if found.cycles.len() >= max_cycles {
                    found.truncated = true;
                    break 'starts;
                }
                if path.len() >= min_size {
                    found
                        .cycles
                        .push(path.iter().map(|ranked| by_name[*ranked]).collect());
                }
            } else if next > start && !on_path[next] && path.len() < max_length {
                path.push(next);
                on_path[next] = true;
                cursors.push(0);
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::bounded_simple_cycles;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn cycles_start_at_their_smallest_name_whatever_the_input_order() {
        let forward = bounded_simple_cycles(
            &names(&["c", "a", "b"]),
            &[(1, 2), (2, 0), (0, 1), (2, 1)],
            2,
            10,
            100,
            1_000,
        );
        // a=1, b=2, c=0: a->b->c->a and a->b->a.
        assert_eq!(forward.cycles, vec![vec![1, 2], vec![1, 2, 0]]);
        assert_eq!((forward.examined, forward.truncated), (2, false));
    }

    #[test]
    fn a_limit_keeps_the_same_prefix_and_counts_the_cycle_that_hit_it() {
        // A complete graph on five nodes has 84 simple cycles of 2+ nodes.
        let list = names(&["e", "d", "c", "b", "a"]);
        let edges: Vec<(usize, usize)> = (0..5)
            .flat_map(|i| (0..5).filter(move |j| *j != i).map(move |j| (i, j)))
            .collect();
        let all = bounded_simple_cycles(&list, &edges, 2, 10, 1_000, 1_000_000);
        assert_eq!((all.cycles.len(), all.truncated), (84, false));
        let capped = bounded_simple_cycles(&list, &edges, 2, 10, 10, 1_000_000);
        assert!(capped.truncated);
        assert_eq!(capped.examined, 11);
        assert_eq!(capped.cycles, all.cycles[..10].to_vec());
        let short = bounded_simple_cycles(&list, &edges, 2, 2, 1_000, 1_000_000);
        assert_eq!(short.cycles.len(), 10, "pairs only");
    }
}
