//! `get_suggested_questions_tool`
//! (`dagayn.tools.analysis_tools.get_suggested_questions_func`): the store's
//! questions, high priority first.

use serde_json::{Map, Value, json};

use crate::analysis::py_prefix;
use crate::arch_tool::analysis_response;
use crate::review_summary::guidance_item;
use crate::{Args, Context, Payload, open_graph, resolve_repo};

const PRIORITIES: [&str; 3] = ["high", "medium", "low"];

pub(crate) fn suggested_questions(
    context: &Context,
    arguments: &Map<String, Value>,
) -> Option<Payload> {
    let args = Args::new(arguments, &["top_n", "repo_root"])?;
    let top_n = args.integer("top_n", 15)?;
    let root = resolve_repo(context, args.optional_string("repo_root")?)?;
    let graph = open_graph(&root)?;
    let answerability = graph.answerability()?;
    let questions: Vec<Value> =
        serde_json::from_str(&graph.store.generate_suggested_questions_json().ok()?).ok()?;

    let mut buckets: [Vec<Value>; 3] = Default::default();
    for question in questions {
        let priority = match question.get("priority") {
            None => "medium",
            Some(Value::String(priority)) => priority.as_str(),
            Some(_) => continue,
        };
        if let Some(at) = PRIORITIES.iter().position(|p| *p == priority) {
            buckets[at].push(question);
        }
    }
    let by_priority: Map<String, Value> = PRIORITIES
        .iter()
        .zip(&buckets)
        .map(|(priority, items)| (priority.to_string(), json!(items.len())))
        .collect();
    let ordered: Vec<Value> = buckets.into_iter().flatten().collect();
    let total = ordered.len();
    let truncated = total as i64 > top_n;
    let returned = py_prefix(&ordered, top_n);

    let guidance = guidance_item(
        format!("Generated {total} review question(s) from graph signals."),
        json!({"type": "computed", "by_priority": by_priority, "returned": returned.len()}),
        if returned.is_empty() { "low" } else { "medium" },
        Vec::new(),
        "review_tool mode=\"changes\" -- apply questions to current changes",
        vec![json!("suggested_questions")],
        json!({"total_questions": total, "returned_questions": returned.len()}),
    );
    let mut summary = format!("Generated {total} review question(s).");
    if truncated {
        summary.push_str(&format!(" Showing top {top_n} (high priority first)."));
    }
    Some(
        analysis_response(
            context,
            &answerability,
            summary,
            vec![
                ("questions", Value::Array(returned)),
                ("total", json!(total)),
                ("truncated", json!(truncated)),
                ("by_priority", Value::Object(by_priority)),
            ],
            guidance,
            &[
                "architecture_analysis_tool mode=\"knowledge_gaps\" -- structural weaknesses",
                "review_tool mode=\"changes\" -- risk-scored review",
                "architecture_analysis_tool mode=\"overview\" -- community map",
            ],
        )
        .put("_repo", graph.repo_context())
        .into_payload(),
    )
}
