//! The effort-estimate JSON contract (`prompts/planner_effort.v1.md`), parsed and
//! reconciled against the topics the caller asked about.
//!
//! Pure: a reply string in, `(topic, minutes)` pairs out. The module returns minutes only —
//! it is told in its own text that the scheduler is deterministic code that runs after it —
//! so nothing here proposes a time, a date or a plan.
//!
//! The reconciliation is the load-bearing half. The module says "Return exactly one entry
//! per input topic, using the same topic names", and a reply that silently drops a topic is
//! a topic the student will never be scheduled to study — a plan that looks complete and is
//! not. So a missing, extra or duplicated topic is a `Schema` error and a retry, never a
//! partial estimate.

use super::{GenerationError, strip_code_fence};

/// The most minutes one topic may be estimated at. Ten hours for a single topic is already
/// beyond anything a weekly plan can place; above this the reply is a model malfunction
/// rather than an estimate, and a wrong number here would silently consume a student's week.
pub const MAX_ESTIMATE_MIN: i64 = 6000;

/// The reply, exactly as `prompts/planner_effort.v1.md` documents it.
///
/// `deny_unknown_fields` is what makes a drifted prompt fail loudly: a model that invents a
/// field produces a parse error and a retry rather than a plan built from a field nobody
/// reads.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffortFile {
    pub topics: Vec<EstimatedTopic>,
    pub total_estimate_min: i64,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EstimatedTopic {
    pub topic: String,
    pub estimate_min: i64,
    pub reason: String,
}

/// Parses a model reply into the estimate, structurally only.
///
/// The rules that relate the reply to what was asked — one entry per requested topic, a
/// total that is the sum — are [`reconcile`]'s, because they cannot be checked without the
/// topic list. Tolerates a fenced code block, like every strict-JSON contract here.
pub fn parse_effort(raw: &str) -> Result<EffortFile, GenerationError> {
    let text = strip_code_fence(raw);
    serde_json::from_str(text).map_err(|error| {
        if error.is_syntax() || error.is_eof() {
            GenerationError::Json(error.to_string())
        } else {
            GenerationError::Schema(error.to_string())
        }
    })
}

/// Checks the reply against the topics the caller asked about, and returns the estimates in
/// the caller's order.
///
/// The caller's order rather than the reply's: the plan is built in the order the student's
/// topics are stored, and taking the model's order would make the same request produce two
/// different plans depending on how the model felt that day.
///
/// Refuses, naming the topic or the total:
///  * a requested topic the reply omits, or one it invents;
///  * a name that appears twice;
///  * an estimate outside `1..=MAX_ESTIMATE_MIN`;
///  * a `total_estimate_min` that is not the sum of the estimates.
pub fn reconcile(
    estimate: &EffortFile,
    requested: &[String],
) -> Result<Vec<(String, i64)>, GenerationError> {
    if requested.is_empty() {
        return Err(GenerationError::Schema(
            "there are no topics to estimate, so the call should not have been made".to_string(),
        ));
    }

    for topic in requested {
        let matches = estimate
            .topics
            .iter()
            .filter(|entry| entry.topic.trim() == topic.trim())
            .count();
        match matches {
            0 => {
                return Err(GenerationError::Schema(format!(
                    "the reply has no estimate for the topic {topic:?}"
                )));
            }
            1 => {}
            count => {
                return Err(GenerationError::Schema(format!(
                    "the reply estimates the topic {topic:?} {count} times; one entry per \
                     topic is the module's rule"
                )));
            }
        }
    }

    for entry in &estimate.topics {
        if !requested
            .iter()
            .any(|topic| topic.trim() == entry.topic.trim())
        {
            return Err(GenerationError::Schema(format!(
                "the reply invents the topic {:?}, which was not asked about",
                entry.topic
            )));
        }
        if !(1..=MAX_ESTIMATE_MIN).contains(&entry.estimate_min) {
            return Err(GenerationError::Schema(format!(
                "the estimate for {:?} is {} minutes, outside 1..={MAX_ESTIMATE_MIN}",
                entry.topic, entry.estimate_min
            )));
        }
    }

    let mut reconciled = Vec::with_capacity(requested.len());
    for topic in requested {
        let entry = estimate
            .topics
            .iter()
            .find(|entry| entry.topic.trim() == topic.trim())
            .ok_or_else(|| {
                GenerationError::Schema(format!("the reply has no estimate for {topic:?}"))
            })?;
        reconciled.push((topic.trim().to_string(), entry.estimate_min));
    }

    let sum: i64 = reconciled.iter().map(|(_, minutes)| *minutes).sum();
    if estimate.total_estimate_min != sum {
        return Err(GenerationError::Schema(format!(
            "total_estimate_min is {}, but the topic estimates sum to {sum}",
            estimate.total_estimate_min
        )));
    }

    Ok(reconciled)
}
