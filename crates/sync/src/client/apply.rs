//! One page of changes, as the writes a store applies.

use std::collections::HashMap;

use uuid::Uuid;

use serde_json::Value;

use crate::proto::{Change, Entity, Op};

/// A page's changes, normalised into the writes a store applies.
#[derive(Debug, Clone, PartialEq)]
pub struct ChangeSet {
    writes: Vec<Change>,
}

impl ChangeSet {
    /// The writes, in the order a store applies them.
    pub fn writes(&self) -> &[Change] {
        &self.writes
    }
}

/// Normalises one page (A188) and orders it so a parent is written before its child.
///
/// Three rules, each of which is a defect if it is missing:
///
///   - an `upsert` whose row this device cannot see arrives as `data: null` and is applied
///     as a **delete**, because a row a device cannot see must never be created on it;
///   - the newest change per row wins: a page is seq-ordered, so an earlier change for the
///     same row would be written and immediately overwritten;
///   - parents come first, by [`Entity::dependency_rank`] — the order the initial pass
///     already uses, and for an incremental page what keeps a child from being deferred
///     behind a parent that is in the same page.
pub fn plan(changes: &[Change]) -> ChangeSet {
    let mut writes: Vec<Change> = Vec::with_capacity(changes.len());
    let mut seen: HashMap<(Entity, Uuid), usize> = HashMap::new();
    for change in changes {
        match seen.get(&(change.entity, change.id)) {
            Some(at) => writes[*at] = change.clone(),
            None => {
                seen.insert((change.entity, change.id), writes.len());
                writes.push(change.clone());
            }
        }
    }

    for change in &mut writes {
        // A payload that carries no row is a delete, whether it arrived as no `data` at all
        // (the wire shape of A188) or as an explicit `null` (a caller that built the change
        // by hand). A row this device cannot see must never be created on it.
        if change.op == Op::Upsert && change.data.as_ref().is_none_or(Value::is_null) {
            change.op = Op::Delete;
        }
    }

    writes.sort_by_key(|change| (change.entity.dependency_rank(), change.seq));
    ChangeSet { writes }
}
