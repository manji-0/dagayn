use crate::*;

impl GraphStore {
    pub fn compute_summaries(&mut self) -> Result<()> {
        match self.compute_risk_index() {
            Ok(()) | Err(GraphError::Sqlite(_)) => {}
            Err(err) => return Err(err),
        }
        Ok(())
    }
}
