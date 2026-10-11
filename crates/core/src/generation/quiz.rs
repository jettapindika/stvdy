//! The quiz JSON contract (`prompts/quiz.v1.md`), parsed and resolved.

use uuid::Uuid;

use crate::citations::ContextLabels;
use crate::generation::{GenerationError, strip_code_fence};
use crate::quiz::normalize;

/// The most questions one reply may carry. Above the largest request the API accepts,
/// so a model that ignores `{{count}}` cannot store an unbounded quiz.
pub const MAX_QUESTIONS_PER_REPLY: usize = 60;

/// How many options an MCQ must have.
///
/// The module says four; this is what makes it enforceable rather than aspirational. A
/// three-option MCQ parses fine as JSON, so nothing but a rule here catches it, and the
/// client's `1`-`4` keyboard shortcut assumes four.
pub const MCQ_OPTIONS: usize = 4;

/// The two kinds, as the module spells them.
pub const MCQ: &str = "mcq";
pub const SHORT: &str = "short";

/// The reply, exactly as `prompts/quiz.v1.md` documents it.
///
/// `deny_unknown_fields` is what makes a drifted prompt fail loudly: a model that
/// invents a field produces a parse error and a retry rather than a question with a
/// silently dropped field.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuestionsFile {
    pub questions: Vec<GeneratedQuestion>,
}

/// One question as the model returned it.
///
/// `options` and `key_points` are `Option` because the module writes an explicit `null`
/// for the field the kind does not use, and which field that is depends on `kind`. The
/// pairing is enforced in [`parse_questions`], not by the type, because serde cannot
/// express "present only when another field says so" without a custom deserialiser, and
/// a hand-written one would be more code than the check it replaces.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeneratedQuestion {
    pub kind: String,
    pub prompt: String,
    pub options: Option<Vec<String>>,
    pub answer: String,
    pub explanation: String,
    /// The chunk label the question came from, as shown inside `<sources>`.
    pub label: u32,
    pub key_points: Option<Vec<String>>,
}

/// One question with its chunk resolved, or `chunk_id: None` when the label was not in
/// the context (A17: dropped, never guessed).
#[derive(Debug, Clone)]
pub struct ResolvedQuestion {
    pub kind: String,
    pub prompt: String,
    pub options: Option<Vec<String>>,
    pub answer: String,
    pub explanation: String,
    pub key_points: Option<Vec<String>>,
    pub chunk_id: Option<Uuid>,
    pub label: u32,
}

#[derive(Debug, Clone, Default)]
pub struct QuizParseOutcome {
    pub questions: Vec<ResolvedQuestion>,
    /// Labels the model cited that were not in the context, deduplicated and sorted.
    /// Recorded rather than silently swallowed: a rate that climbs means the prompt or
    /// the model changed.
    pub dropped_labels: Vec<u32>,
}

/// Parses a model reply into questions.
///
/// Tolerates a fenced code block, because a provider occasionally adds one despite the
/// contract. It does **not** tolerate prose around the object: guessing which part is
/// the JSON is how a half-parsed quiz ships.
pub fn parse_questions(raw: &str) -> Result<Vec<GeneratedQuestion>, GenerationError> {
    let text = strip_code_fence(raw);
    let file: QuestionsFile = serde_json::from_str(text).map_err(|error| {
        // `serde_json` reports both "not JSON" and "wrong shape" through the same error
        // type; the message distinguishes them, and the caller only needs to know a
        // retry is worth one attempt.
        if error.is_syntax() || error.is_eof() {
            GenerationError::Json(error.to_string())
        } else {
            GenerationError::Schema(error.to_string())
        }
    })?;

    if file.questions.is_empty() {
        // The model answered the question it was asked and the answer was "no
        // questions". Retrying would spend a second call to be told the same thing.
        return Err(GenerationError::Empty);
    }
    if file.questions.len() > MAX_QUESTIONS_PER_REPLY {
        return Err(GenerationError::Schema(format!(
            "the model returned {} questions, more than the {MAX_QUESTIONS_PER_REPLY} allowed",
            file.questions.len()
        )));
    }

    for (index, question) in file.questions.iter().enumerate() {
        validate(index, question)?;
    }

    Ok(file.questions)
}

/// Checks the pairing the type cannot express.
///
/// The index is in every message so the retry's corrective text can name the question
/// that was wrong, which is the difference between a model fixing it and a model
/// guessing again.
fn validate(index: usize, question: &GeneratedQuestion) -> Result<(), GenerationError> {
    match question.kind.as_str() {
        MCQ => {
            let options = question.options.as_ref().ok_or_else(|| {
                GenerationError::Schema(format!("question {index}: an mcq must carry options"))
            })?;
            if options.len() != MCQ_OPTIONS {
                return Err(GenerationError::Schema(format!(
                    "question {index}: an mcq must carry exactly {MCQ_OPTIONS} options, not {}",
                    options.len()
                )));
            }
            if options.iter().any(|option| option.trim().is_empty()) {
                return Err(GenerationError::Schema(format!(
                    "question {index}: an mcq option is empty"
                )));
            }
            // The answer must be one of the options, or the server cannot grade this
            // question at all: it compares the submitted option text to `answer`.
            if !options
                .iter()
                .any(|option| normalize(option) == normalize(&question.answer))
            {
                return Err(GenerationError::Schema(format!(
                    "question {index}: an mcq's answer must be one of its options, copied \
                     verbatim; the module documents this and the grader depends on it"
                )));
            }
        }
        SHORT => {
            if question.options.is_some() {
                return Err(GenerationError::Schema(format!(
                    "question {index}: a short answer carries no options"
                )));
            }
            match question.key_points.as_ref() {
                Some(points) if !points.is_empty() => {}
                _ => {
                    return Err(GenerationError::Schema(format!(
                        "question {index}: a short answer must carry key_points, because a \
                         student cannot self-assess against a model answer alone"
                    )));
                }
            }
        }
        other => {
            return Err(GenerationError::Schema(format!(
                "question {index}: kind must be `{MCQ}` or `{SHORT}`, not `{other}`"
            )));
        }
    }

    if question.prompt.trim().is_empty() {
        return Err(GenerationError::Schema(format!(
            "question {index}: the prompt is empty"
        )));
    }
    if question.answer.trim().is_empty() {
        return Err(GenerationError::Schema(format!(
            "question {index}: the answer is empty"
        )));
    }
    Ok(())
}

/// Resolves each question's label against the labels the prompt actually sent, and
/// drops the questions that carry nothing to store.
pub fn resolve_questions(
    questions: Vec<GeneratedQuestion>,
    labels: &ContextLabels,
) -> QuizParseOutcome {
    let mut outcome = QuizParseOutcome::default();

    for question in questions {
        let prompt = question.prompt.trim().to_string();
        let answer = question.answer.trim().to_string();
        if prompt.is_empty() || answer.is_empty() {
            continue;
        }

        let chunk_id = labels.chunk(question.label).map(|chunk| chunk.chunk_id);
        if chunk_id.is_none() && !outcome.dropped_labels.contains(&question.label) {
            outcome.dropped_labels.push(question.label);
        }

        outcome.questions.push(ResolvedQuestion {
            kind: question.kind,
            prompt,
            options: question.options,
            answer,
            explanation: question.explanation.trim().to_string(),
            key_points: question.key_points,
            chunk_id,
            label: question.label,
        });
    }

    outcome.dropped_labels.sort_unstable();
    outcome
}
