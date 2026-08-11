use crate::helpers::*;
use crate::*;

impl GraphStore {
    pub(crate) fn init_schema(&self) -> Result<()> {
        self.conn.execute_batch(SCHEMA_SQL)?;
        Ok(())
    }

    pub(crate) fn run_migrations(&self) -> Result<()> {
        let current = self.schema_version()?;
        if current < LATEST_VERSION {
            for version in (current + 1)..=LATEST_VERSION {
                match version {
                    2 => self.migrate_v2()?,
                    3 => self.migrate_v3()?,
                    4 => self.migrate_v4()?,
                    5 => self.migrate_v5()?,
                    6 => self.migrate_v6()?,
                    7 => self.migrate_v7()?,
                    8 => self.migrate_v8()?,
                    9 => self.migrate_v9()?,
                    10 => self.migrate_v10()?,
                    11 => self.migrate_v11()?,
                    12 => self.migrate_v12()?,
                    13 => self.migrate_v13()?,
                    14 => self.migrate_v14()?,
                    15 => self.migrate_v15()?,
                    _ => {}
                }
                self.set_metadata("schema_version", &version.to_string())?;
            }
        }
        self.ensure_edge_target_name_column()?;
        Ok(())
    }

    pub(crate) fn get_edges_by_endpoint(
        &self,
        column: &str,
        qualified_name: &str,
    ) -> Result<Vec<GraphEdge>> {
        let mut seen = std::collections::HashSet::<i64>::new();
        let mut edges = Vec::new();
        let sql = format!("SELECT * FROM edges WHERE {column} = ?");
        for key in self.qualified_key_candidates(qualified_name)? {
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map([key], edge_from_row)?;
            for row in rows {
                let edge = row?;
                if seen.insert(edge.id) {
                    edges.push(edge);
                }
            }
        }
        Ok(edges)
    }

    pub(crate) fn file_key_candidates(&self, file_path: &str) -> Result<Vec<String>> {
        let normalized = self.normalize_file_path_key(file_path)?;
        if normalized == file_path {
            Ok(vec![file_path.to_string()])
        } else {
            Ok(vec![file_path.to_string(), normalized])
        }
    }

    pub(crate) fn qualified_key_candidates(&self, qualified_name: &str) -> Result<Vec<String>> {
        let normalized = self.normalize_qualified_key(qualified_name)?;
        if normalized == qualified_name {
            Ok(vec![qualified_name.to_string()])
        } else {
            Ok(vec![qualified_name.to_string(), normalized])
        }
    }

    pub(crate) fn normalize_qualified_key(&self, qualified_name: &str) -> Result<String> {
        if let Some((file_path, rest)) = qualified_name.split_once("::") {
            Ok(format!(
                "{}::{rest}",
                self.normalize_file_path_key(file_path)?
            ))
        } else {
            self.normalize_file_path_key(qualified_name)
        }
    }

    pub(crate) fn normalize_file_path_key(&self, file_path: &str) -> Result<String> {
        let path = Path::new(file_path);
        if !path.is_absolute() {
            return Ok(file_path.to_string());
        }
        let Some(repo_root) = self.get_metadata("repo_root")? else {
            return Ok(file_path.to_string());
        };
        let repo_root = Path::new(&repo_root);
        if let Ok(rel) = path.strip_prefix(repo_root) {
            return Ok(rel.to_string_lossy().to_string());
        }
        if let (Ok(path), Ok(repo_root)) = (path.canonicalize(), repo_root.canonicalize()) {
            if let Ok(rel) = path.strip_prefix(repo_root) {
                return Ok(rel.to_string_lossy().to_string());
            }
        }
        Ok(file_path.to_string())
    }
}
