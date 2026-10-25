use crate::helpers::*;
use crate::*;

/// A bare Terraform reference bound to the only same-module declaration.
const TERRAFORM_MODULE_SCOPE_CONFIDENCE: f64 = 0.8;

/// The Terraform module a file belongs to: its directory (`""` at the root).
fn terraform_module_dir(file_path: &str) -> &str {
    file_path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

impl GraphStore {
    /// Qualify bare Terraform `REFERENCES` targets declared in another file
    /// of the same module.
    ///
    /// Terraform merges every file of one directory into a single module, so
    /// `var.region` in `main.tf` refers to the `variable "region"` block in
    /// `variables.tf`. The parser only qualifies names defined in the file it
    /// is parsing; this step binds the remaining bare targets to the unique
    /// Terraform node with that name in the edge's directory. Ambiguous
    /// names (for example an `override.tf` redefining a block) stay bare.
    pub fn resolve_terraform_module_references(&mut self) -> Result<i64> {
        let tx = write_tx(&mut self.conn)?;
        let index = {
            let mut stmt = tx.prepare(
                "SELECT name, qualified_name, file_path FROM nodes \
                 WHERE language = 'terraform' AND kind != 'File'",
            )?;
            let mapped = stmt.query_map([], |row| <(String, String, String)>::try_from(row))?;
            let mut index = HashMap::<String, Vec<(String, String)>>::new();
            for row in mapped {
                let (name, qualified_name, file_path) = row?;
                index
                    .entry(name)
                    .or_default()
                    .push((terraform_module_dir(&file_path).to_string(), qualified_name));
            }
            index
        };
        let edges = {
            let mut stmt = tx.prepare(
                "SELECT e.id, e.target_qualified, e.file_path FROM edges e \
                 WHERE e.kind = 'REFERENCES' AND e.target_qualified NOT LIKE '%::%' \
                 AND EXISTS (SELECT 1 FROM nodes f WHERE f.qualified_name = e.file_path \
                             AND f.kind = 'File' AND f.language = 'terraform')",
            )?;
            let mapped = stmt.query_map([], |row| <(i64, String, String)>::try_from(row))?;
            mapped.collect::<std::result::Result<Vec<_>, _>>()?
        };
        let mut resolved = 0_i64;
        for (edge_id, target, file_path) in edges {
            let Some(candidates) = index.get(&target) else {
                continue;
            };
            let module_dir = terraform_module_dir(&file_path);
            let mut in_module = candidates.iter().filter(|(dir, _)| dir == module_dir);
            let (Some((_, qualified)), None) = (in_module.next(), in_module.next()) else {
                continue;
            };
            tx.execute(
                "UPDATE edges SET target_qualified = ?, target_name = ?, \
                 confidence = ?, confidence_tier = ? WHERE id = ?",
                params![
                    qualified,
                    edge_target_name(qualified),
                    TERRAFORM_MODULE_SCOPE_CONFIDENCE,
                    ConfidenceTier::High.as_str(),
                    edge_id
                ],
            )?;
            resolved += 1;
        }
        tx.commit()?;
        Ok(resolved)
    }
}
