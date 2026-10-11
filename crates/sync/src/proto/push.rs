use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::entities::{Entity, Op};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Mutation {
    /// Idempotency key: replaying the same `mutation_id` must not apply twice.
    pub mutation_id: Uuid,
    pub entity: Entity,
    pub op: Op,
    pub id: Uuid,
    pub payload: Option<Value>,
    pub client_ts: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PushRequest {
    pub device_id: Uuid,
    pub mutations: Vec<Mutation>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum MutationResult {
    Applied { seq: i64 },
    Rejected { code: String, message: String },
    Conflict { winner: Value },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PushResponse {
    pub results: Vec<MutationResult>,
}
