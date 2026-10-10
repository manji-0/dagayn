//! Community rows and their JSON shape.

use super::*;

#[derive(Serialize)]
struct CommunityJson {
    id: i64,
    name: String,
    level: i64,
    cohesion: f64,
    size: i64,
    dominant_language: String,
    description: String,
    members: Vec<String>,
}

pub(crate) fn community_json_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    let name: String = row.get("name")?;
    let description = row
        .get::<_, Option<String>>("description")?
        .unwrap_or_default();
    Ok(json!(CommunityJson {
        id: row.get::<_, i64>("id")?,
        name: sanitize_name(&name),
        level: row.get::<_, i64>("level")?,
        cohesion: row.get::<_, f64>("cohesion")?,
        size: row.get::<_, i64>("size")?,
        dominant_language: row
            .get::<_, Option<String>>("dominant_language")?
            .unwrap_or_default(),
        description: sanitize_name(&description),
        members: Vec::new(),
    }))
}
