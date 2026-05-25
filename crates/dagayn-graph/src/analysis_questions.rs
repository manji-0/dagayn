use crate::helpers::*;
use crate::*;

impl GraphStore {
    pub fn generate_suggested_questions_json(&self) -> Result<String> {
        let nodes = self.get_question_nodes()?;
        let edges = self.get_question_edges()?;
        let community_map = self.get_all_node_community_ids()?;
        let mut tested_sources = HashSet::<String>::new();
        let mut degree = HashMap::<String, i64>::new();
        for edge in &edges {
            *degree.entry(edge.source_qualified.clone()).or_insert(0) += 1;
            *degree.entry(edge.target_qualified.clone()).or_insert(0) += 1;
            if edge.kind == "TESTED_BY" {
                tested_sources.insert(edge.source_qualified.clone());
            }
        }
        let mut questions = Vec::<Value>::new();

        for bridge in self.get_persisted_bridge_rows(3)? {
            questions.push(json!({
                "category": "bridge_node",
                "question": format!(
                    "'{}' is a critical connector between multiple code regions. Is it adequately tested and documented?",
                    bridge.name
                ),
                "target": bridge.qualified_name,
                "priority": "high",
            }));
        }

        for hub in self.get_persisted_hub_rows(3)? {
            if tested_sources.contains(&hub.qualified_name) {
                continue;
            }
            questions.push(json!({
                "category": "hub_risk",
                "question": format!(
                    "Hub node '{}' has {} connections but no direct test coverage. Should it be tested?",
                    hub.name, hub.total_degree
                ),
                "target": hub.qualified_name,
                "priority": "high",
            }));
        }

        for surprise in
            self.find_surprising_connection_questions(3, &nodes, &edges, &community_map, &degree)
        {
            questions.push(surprise);
        }

        let gaps =
            self.find_question_gap_inputs(&nodes, &community_map, &degree, &tested_sources)?;
        for community in gaps.thin_communities.into_iter().take(2) {
            questions.push(json!({
                "category": "thin_community",
                "question": format!(
                    "Community '{}' has only {} member(s). Should it be merged with a neighbor?",
                    community.name, community.size
                ),
                "target": format!("community:{}", community.id),
                "priority": "low",
            }));
        }
        for hotspot in gaps.untested_hotspots.into_iter().take(2) {
            questions.push(json!({
                "category": "untested_hotspot",
                "question": format!(
                    "'{}' has {} connections but no test coverage. Is this a risk?",
                    hotspot.name, hotspot.degree
                ),
                "target": hotspot.qualified_name,
                "priority": "medium",
            }));
        }

        serde_json::to_string(&questions).map_err(Into::into)
    }

    pub(crate) fn find_surprising_connection_questions(
        &self,
        limit: usize,
        nodes: &[QuestionNode],
        edges: &[QuestionEdge],
        community_map: &HashMap<String, i64>,
        degree: &HashMap<String, i64>,
    ) -> Vec<Value> {
        let node_map = nodes
            .iter()
            .map(|node| (node.qualified_name.clone(), node))
            .collect::<HashMap<_, _>>();

        let mut scored = Vec::<SurprisingQuestionInput>::new();
        for edge in edges {
            let Some(source) = node_map.get(&edge.source_qualified) else {
                continue;
            };
            let Some(target) = node_map.get(&edge.target_qualified) else {
                continue;
            };
            let Some(source_community) = community_map.get(&edge.source_qualified).copied() else {
                continue;
            };
            let Some(target_community) = community_map.get(&edge.target_qualified).copied() else {
                continue;
            };
            if source_community == target_community {
                continue;
            }
            let source_degree = *degree.get(&edge.source_qualified).unwrap_or(&0);
            let target_degree = *degree.get(&edge.target_qualified).unwrap_or(&0);
            scored.push(SurprisingQuestionInput {
                source_name: sanitize_name(&source.name),
                source_qualified: edge.source_qualified.clone(),
                target_name: sanitize_name(&target.name),
                source_community,
                target_community,
                score: source_degree + target_degree,
            });
        }
        scored.sort_by(|left, right| {
            right
                .score
                .cmp(&left.score)
                .then_with(|| left.source_qualified.cmp(&right.source_qualified))
        });

        scored
            .into_iter()
            .take(limit)
            .map(|item| {
                json!({
                    "category": "surprising_connection",
                    "question": format!(
                        "'{}' (community {}) calls '{}' (community {}). Is this coupling intentional?",
                        item.source_name,
                        item.source_community,
                        item.target_name,
                        item.target_community
                    ),
                    "target": item.source_qualified,
                    "priority": "medium",
                })
            })
            .collect()
    }

    pub(crate) fn find_question_gap_inputs(
        &self,
        nodes: &[QuestionNode],
        community_map: &HashMap<String, i64>,
        degree: &HashMap<String, i64>,
        tested_sources: &HashSet<String>,
    ) -> Result<QuestionGaps> {
        let mut positive_degrees = nodes
            .iter()
            .filter(|node| !is_analysis_excluded_from_test_gap(node))
            .filter_map(|node| {
                let value = *degree.get(&node.qualified_name).unwrap_or(&0);
                (value > 0).then_some(value)
            })
            .collect::<Vec<_>>();
        positive_degrees.sort_unstable();
        let degree_p95 = nearest_rank_percentile(&positive_degrees, 0.95);
        let hotspot_min_degree = 5.max(degree_p95);

        let mut community_sizes = HashMap::<i64, i64>::new();
        for node in nodes {
            if let Some(community_id) = community_map.get(&node.qualified_name) {
                *community_sizes.entry(*community_id).or_insert(0) += 1;
            }
        }

        let mut thin_communities = Vec::new();
        let mut stmt = self
            .conn
            .prepare("SELECT id, name FROM communities ORDER BY id")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (id, name) = row?;
            let size = *community_sizes.get(&id).unwrap_or(&0);
            if size < 3 {
                thin_communities.push(QuestionCommunity { id, name, size });
            }
        }

        let mut untested_hotspots = Vec::new();
        for node in nodes {
            let node_degree = *degree.get(&node.qualified_name).unwrap_or(&0);
            if node_degree >= hotspot_min_degree
                && !tested_sources.contains(&node.qualified_name)
                && !is_analysis_excluded_from_test_gap(node)
            {
                untested_hotspots.push(QuestionHotspot {
                    name: sanitize_name(&node.name),
                    qualified_name: node.qualified_name.clone(),
                    degree: node_degree,
                });
            }
        }
        untested_hotspots.sort_by(|left, right| {
            right
                .degree
                .cmp(&left.degree)
                .then_with(|| left.qualified_name.cmp(&right.qualified_name))
        });

        Ok(QuestionGaps {
            thin_communities,
            untested_hotspots,
        })
    }
}
