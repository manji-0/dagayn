//! Betweenness centrality and the persisted hub / bridge rows.

use super::*;

pub(crate) const BETWEENNESS_EXACT_LIMIT: usize = 5000;

pub(crate) const BETWEENNESS_SAMPLE_CAP: usize = 500;

pub(crate) const BETWEENNESS_SAMPLE_FLOOR: usize = 64;

pub(crate) const BETWEENNESS_SAMPLE_SQRT_C: f64 = 5.0;

/// Brandes source count: exact on small graphs, otherwise
/// `min(500, max(64, ceil(5 * sqrt(V))))`.
pub(crate) fn betweenness_sample_size(node_count: usize) -> usize {
    if node_count <= BETWEENNESS_EXACT_LIMIT {
        return node_count;
    }
    let scaled = (BETWEENNESS_SAMPLE_SQRT_C * (node_count as f64).sqrt()).ceil() as usize;
    scaled
        .clamp(BETWEENNESS_SAMPLE_FLOOR, BETWEENNESS_SAMPLE_CAP)
        .min(node_count)
}

/// Integer-indexed directed graph used by Brandes (and reusable by other
/// postprocess steps that need adjacency without `HashMap<String, …>`).
pub(crate) struct DenseGraph {
    pub names: Box<[String]>,
    pub adj: Box<[Box<[usize]>]>,
}

impl DenseGraph {
    pub(crate) fn from_adjacency(
        graph_nodes: &HashSet<String>,
        adjacency: &HashMap<String, Vec<String>>,
    ) -> Self {
        let mut names = graph_nodes.iter().cloned().collect::<Vec<_>>();
        names.sort();
        let node_count = names.len();
        let mut index_of = HashMap::<&str, usize>::with_capacity(node_count);
        for (idx, name) in names.iter().enumerate() {
            index_of.insert(name.as_str(), idx);
        }
        let mut adj = vec![Vec::<usize>::new(); node_count];
        for (source, targets) in adjacency {
            let Some(&src) = index_of.get(source.as_str()) else {
                continue;
            };
            for target in targets {
                if let Some(&tgt) = index_of.get(target.as_str()) {
                    adj[src].push(tgt);
                }
            }
        }
        Self {
            names: names.into_boxed_slice(),
            adj: adj.into_iter().map(Vec::into_boxed_slice).collect(),
        }
    }

    pub(crate) fn betweenness(&self) -> HashMap<String, f64> {
        betweenness_on_dense(self)
    }
}

pub(crate) fn betweenness_centrality(
    graph_nodes: &HashSet<String>,
    adjacency: &HashMap<String, Vec<String>>,
) -> HashMap<String, f64> {
    DenseGraph::from_adjacency(graph_nodes, adjacency).betweenness()
}

fn betweenness_on_dense(graph: &DenseGraph) -> HashMap<String, f64> {
    let names = &graph.names;
    let adj = &graph.adj;
    let node_count = names.len();
    if node_count == 0 {
        return HashMap::new();
    }

    let sample_size = betweenness_sample_size(node_count);
    let sources: Vec<usize> = if sample_size >= node_count {
        (0..node_count).collect()
    } else {
        let mut index_of = HashMap::<&str, usize>::with_capacity(node_count);
        for (idx, name) in names.iter().enumerate() {
            index_of.insert(name.as_str(), idx);
        }
        deterministic_centrality_sample(names, sample_size)
            .iter()
            .filter_map(|name| index_of.get(name.as_str()).copied())
            .collect()
    };
    let scale = if sample_size >= node_count {
        1.0
    } else {
        node_count as f64 / sources.len().max(1) as f64
    };

    let mut centrality = vec![0.0_f64; node_count];
    let mut sigma = vec![0.0_f64; node_count];
    let mut distance = vec![-1_i32; node_count];
    let mut dependency = vec![0.0_f64; node_count];
    let mut predecessors = vec![Vec::<usize>::new(); node_count];
    let mut seen = vec![0_u32; node_count];
    let mut generation = 0_u32;

    for source in sources {
        generation = generation.wrapping_add(1);
        if generation == 0 {
            seen.fill(0);
            generation = 1;
        }

        let mut stack = Vec::<usize>::new();
        seen[source] = generation;
        sigma[source] = 1.0;
        distance[source] = 0;
        predecessors[source].clear();

        let mut queue = VecDeque::from([source]);
        while let Some(vertex) = queue.pop_front() {
            stack.push(vertex);
            let vertex_distance = distance[vertex];
            let vertex_sigma = sigma[vertex];
            for &successor in &adj[vertex] {
                if seen[successor] != generation {
                    seen[successor] = generation;
                    distance[successor] = vertex_distance + 1;
                    sigma[successor] = 0.0;
                    predecessors[successor].clear();
                    queue.push_back(successor);
                }
                if distance[successor] == vertex_distance + 1 {
                    sigma[successor] += vertex_sigma;
                    predecessors[successor].push(vertex);
                }
            }
        }

        for &vertex in &stack {
            dependency[vertex] = 0.0;
        }
        while let Some(w) = stack.pop() {
            let sigma_w = sigma[w];
            if sigma_w != 0.0 {
                for &v in &predecessors[w] {
                    dependency[v] += (sigma[v] / sigma_w) * (1.0 + dependency[w]);
                }
            }
            if w != source {
                centrality[w] += dependency[w] * scale;
            }
        }
    }

    if node_count > 2 {
        let norm = 1.0 / ((node_count as f64 - 1.0) * (node_count as f64 - 2.0));
        for value in &mut centrality {
            *value *= norm;
        }
    }

    names.iter().cloned().zip(centrality).collect()
}

pub(crate) fn deterministic_centrality_sample(nodes: &[String], sample_size: usize) -> Vec<String> {
    let mut ranked = nodes
        .iter()
        .map(|node| (stable_fnv1a64(node.as_bytes()), node.clone()))
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    ranked
        .into_iter()
        .take(sample_size.min(nodes.len()))
        .map(|(_, node)| node)
        .collect()
}

pub(crate) fn stable_fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

pub(crate) struct PersistedBridgeRow {
    pub(crate) name: String,
    pub(crate) qualified_name: String,
}

pub(crate) struct PersistedHubRow {
    pub(crate) name: String,
    pub(crate) qualified_name: String,
    pub(crate) total_degree: i64,
}
