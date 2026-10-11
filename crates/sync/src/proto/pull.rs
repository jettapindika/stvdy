use serde::{Deserialize, Serialize};

use super::entities::Change;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PullResponse {
    pub changes: Vec<Change>,
    pub next_cursor: i64,
    pub has_more: bool,
}
