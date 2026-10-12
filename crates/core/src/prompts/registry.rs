//! Compiled-in prompt modules.
//!
//! `include_str!` means a missing or renamed prompt file is a build error, not
//! a runtime 500 in front of a user. The version string is the filename stem,
//! and it is what lands in `usage_events.prompt_version` so a quality
//! regression can be attributed to a prompt change rather than guessed at.

use super::VarKind;

/// The chat base prompt. Every chat and assistant turn is composed on top of
/// this text, never on inline prompt strings scattered through the server.
pub const BASE_V1: &str = include_str!("../../../../prompts/base.v1.md");

/// The version stored with every request composed from [`BASE_V1`].
pub const BASE_V1_VERSION: &str = "base.v1";

/// The chat base prompt, v2. Byte-identical to [`BASE_V1`] except that its
/// `<grounding_and_citations>` rules honour the per-chat "Answer only from my
/// sources" toggle through `{{sources_only}}`.
///
/// A38: a shipped version is never edited, only superseded. `base.v1.md` stays
/// compiled in so task modules (which keep composing on v1's trimmed base) and
/// any `usage_events` row that named it remain attributable.
pub const BASE_V2: &str = include_str!("../../../../prompts/base.v2.md");

/// The version stored with every request composed from [`BASE_V2`].
pub const BASE_V2_VERSION: &str = "base.v2";

/// The shared strict-JSON preamble every task module is followed by.
pub const JSON_CONTRACT_V1: &str = include_str!("../../../../prompts/json_contract.v1.md");

/// The flashcards module.
pub const FLASHCARDS_V1: &str = include_str!("../../../../prompts/flashcards.v1.md");

/// The quiz module.
pub const QUIZ_V1: &str = include_str!("../../../../prompts/quiz.v1.md");

/// The report outline module. The first half of a report: the shape the student
/// edits before any section is written.
pub const REPORT_OUTLINE_V1: &str = include_str!("../../../../prompts/report_outline.v1.md");

/// The report section module. One call per section, so a student can regenerate
/// section 3 without paying for the whole report again.
pub const REPORT_SECTION_V1: &str = include_str!("../../../../prompts/report_section.v1.md");

/// The cheatsheet module. Its reply is parsed by [`crate::render::blocks`], the
/// renderer's own block model, so the contract that accepts a reply and the contract
/// that renders it are one thing.
pub const CHEATSHEET_V1: &str = include_str!("../../../../prompts/cheatsheet.v1.md");

/// The quick-capture module. No sources: the input is one line the student typed, so
/// the prompt has no material to detect a language from and the caller states it.
pub const QUICK_CAPTURE_V1: &str = include_str!("../../../../prompts/quick_capture.v1.md");

/// The task-breakdown module. No sources either: the input is a task the student wrote.
pub const TASK_BREAKDOWN_V1: &str = include_str!("../../../../prompts/task_breakdown.v1.md");

/// The syllabus-extraction module. The syllabus document is the source, so the module
/// composes with a `<sources>` block like every other document task.
pub const SYLLABUS_V1: &str = include_str!("../../../../prompts/syllabus.v1.md");

/// The effort-estimate module. The one model call the planner makes, and it returns minutes
/// only: the scheduling itself is deterministic code that runs after it.
pub const PLANNER_EFFORT_V1: &str = include_str!("../../../../prompts/planner_effort.v1.md");

/// What `{{output_language}}` means when the caller supplies nothing.
///
/// A study app's cards must be in the language of the student's material. There is
/// no language setting until Phase 9, and translating a deck into English would be
/// a silent content bug for every Indonesian student, so the default names the
/// material rather than picking a language.
pub const SOURCE_LANGUAGE: &str = "the same language as the source material (do not translate)";

/// What `{{other_section_headings}}` means when there is no other section yet.
///
/// The first section of every report has nothing to avoid repeating, so absence here is a
/// normal state and not a caller mistake: `prompts/README.md` declares the variable
/// Optional for exactly that reason. The sentence is prompt content, so it lives with the
/// prompt rather than being invented by each call site, where two call sites would end up
/// saying two different things to the model.
pub const NO_OTHER_SECTIONS: &str = "(no other sections yet)";

/// What `{{exam_date}}` means when the student has not set one.
///
/// `cheatsheets.exam_date` is nullable, so "no date" is a state the schema models rather
/// than an omission, and the prompt has to be able to say it: a `Caller` variable cannot
/// be empty, so the alternative is every call site inventing its own placeholder for the
/// same state.
pub const NO_EXAM_DATE: &str = "not set";

/// What `{{semester_context}}` means when the student's upload carries no semester label.
///
/// A syllabus often does not name its semester, and the module's date-resolution rule needs
/// something to read rather than an empty string: `render_with` treats an empty `Optional`
/// value as a missing variable, and the alternative is every call site inventing its own
/// placeholder for the same state.
pub const NO_SEMESTER_CONTEXT: &str = "no semester label was provided with the document";

/// The `base.v1.md` sections a strict-JSON task keeps.
///
/// The three dropped by name in `prompts/README.md` (`teaching_style`,
/// `tools_and_actions`, `planning_help`) all invite prose in a call that must
/// return one JSON object. `plans_and_billing` and `when_things_go_wrong` are
/// dropped for the same reason: both are instructions about what to *say* to the
/// user, and there is no user-facing turn here. The safety, integrity, privacy and
/// language sections stay: a source document can contain anything, and the model
/// must still refuse to reproduce what it must not.
pub const TASK_BASE_SECTIONS: &[&str] = &[
    "context",
    "user_context_data",
    "language_and_tone",
    "grounding_and_citations",
    "security_untrusted_content",
    "academic_integrity",
    "wellbeing_and_safety",
    "privacy_and_identity",
];

/// A strict-JSON task module: its text, its variables, and the base sections it
/// keeps. One variant per file under `prompts/`, added here and in [`ModuleId::ALL`]
/// when a phase adds a module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleId {
    FlashcardsV1,
    QuizV1,
    ReportOutlineV1,
    ReportSectionV1,
    CheatsheetV1,
    QuickCaptureV1,
    TaskBreakdownV1,
    SyllabusV1,
    PlannerEffortV1,
}

impl ModuleId {
    /// Every module, for the contract sweep in `tests/prompts.rs`.
    pub const ALL: [ModuleId; 9] = [
        ModuleId::FlashcardsV1,
        ModuleId::QuizV1,
        ModuleId::ReportOutlineV1,
        ModuleId::ReportSectionV1,
        ModuleId::CheatsheetV1,
        ModuleId::QuickCaptureV1,
        ModuleId::TaskBreakdownV1,
        ModuleId::SyllabusV1,
        ModuleId::PlannerEffortV1,
    ];

    /// The version string logged to `usage_events.prompt_version`.
    pub fn version(self) -> &'static str {
        match self {
            ModuleId::FlashcardsV1 => "flashcards.v1",
            ModuleId::QuizV1 => "quiz.v1",
            ModuleId::ReportOutlineV1 => "report_outline.v1",
            ModuleId::ReportSectionV1 => "report_section.v1",
            ModuleId::CheatsheetV1 => "cheatsheet.v1",
            ModuleId::QuickCaptureV1 => "quick_capture.v1",
            ModuleId::TaskBreakdownV1 => "task_breakdown.v1",
            ModuleId::SyllabusV1 => "syllabus.v1",
            ModuleId::PlannerEffortV1 => "planner_effort.v1",
        }
    }

    pub fn text(self) -> &'static str {
        match self {
            ModuleId::FlashcardsV1 => FLASHCARDS_V1,
            ModuleId::QuizV1 => QUIZ_V1,
            ModuleId::ReportOutlineV1 => REPORT_OUTLINE_V1,
            ModuleId::ReportSectionV1 => REPORT_SECTION_V1,
            ModuleId::CheatsheetV1 => CHEATSHEET_V1,
            ModuleId::QuickCaptureV1 => QUICK_CAPTURE_V1,
            ModuleId::TaskBreakdownV1 => TASK_BREAKDOWN_V1,
            ModuleId::SyllabusV1 => SYLLABUS_V1,
            ModuleId::PlannerEffortV1 => PLANNER_EFFORT_V1,
        }
    }

    /// The variables the module's own text (and the JSON contract) uses.
    ///
    /// No module text uses a `base.v1.md` variable, so nothing here is left undeclared on
    /// the grounds that the base supplies it: the trimmed base is rendered against
    /// [`crate::prompts::base_variables`], and a name that only appears in the module would
    /// be an unfilled `{{…}}` in the prompt the model reads.
    pub fn variables(self) -> &'static [(&'static str, VarKind)] {
        match self {
            ModuleId::FlashcardsV1 => &[
                ("count", VarKind::Caller),
                ("output_language", VarKind::Optional(SOURCE_LANGUAGE)),
            ],
            // `difficulty` and `types` are `Caller` rather than `Optional` on purpose: a
            // default would make "mixed, medium" the silent meaning of an absent
            // setting, and what the student got would differ from what they asked for.
            ModuleId::QuizV1 => &[
                ("count", VarKind::Caller),
                ("difficulty", VarKind::Caller),
                ("types", VarKind::Caller),
                ("output_language", VarKind::Optional(SOURCE_LANGUAGE)),
            ],
            // `topic` is what the report is about, and `style` and `length` are the two
            // settings the student chose on the same screen. All three are `Caller`: a
            // default would let a report come out in a length or register nobody asked
            // for, which reads as a model quality problem rather than a missing value.
            ModuleId::ReportOutlineV1 => &[
                ("topic", VarKind::Caller),
                ("style", VarKind::Caller),
                ("length", VarKind::Caller),
                ("output_language", VarKind::Optional(SOURCE_LANGUAGE)),
            ],
            // `heading`, `title` and `purpose` come from the outline row the student may
            // have edited, so the caller has them exactly; a default would substitute a
            // section of the report the student did not ask for. `other_section_headings`
            // is the one exception: the first section of a report genuinely has none.
            ModuleId::ReportSectionV1 => &[
                ("heading", VarKind::Caller),
                ("title", VarKind::Caller),
                ("purpose", VarKind::Caller),
                ("style", VarKind::Caller),
                ("length", VarKind::Caller),
                (
                    "other_section_headings",
                    VarKind::Optional(NO_OTHER_SECTIONS),
                ),
                ("output_language", VarKind::Optional(SOURCE_LANGUAGE)),
            ],
            // `depth` is stored non-null with a `normal` default in `cheatsheets`, so the
            // caller always has the value the student's request resolved to and there is
            // nothing for a prompt default to stand in for. `exam_date` is nullable, and
            // is the one state the prompt has to be able to say.
            ModuleId::CheatsheetV1 => &[
                ("scope", VarKind::Caller),
                ("exam_date", VarKind::Optional(NO_EXAM_DATE)),
                ("depth", VarKind::Caller),
                ("output_language", VarKind::Optional(SOURCE_LANGUAGE)),
            ],
            // `now` and `timezone` are `Caller` and filled by `values_of(ChatVars)`, so
            // the caller passes nothing for them and they still cannot be left unfilled.
            // `output_language` is `Caller` rather than `Optional(SOURCE_LANGUAGE)` because
            // this module has no sources: see A192.
            ModuleId::QuickCaptureV1 => &[
                ("now", VarKind::Caller),
                ("timezone", VarKind::Caller),
                ("capture_text", VarKind::Caller),
                ("output_language", VarKind::Caller),
            ],
            // `due_at` and `notes` are `Caller` and not `Optional`: the module renders
            // `Due: {{due_at}}` and `Notes: {{notes}}` as lines the model reads, and the
            // caller has the values (a task with no due date has the string the caller
            // chooses — the route passes `"none"`, which is a statement the prompt can
            // read, not an absence).
            ModuleId::TaskBreakdownV1 => &[
                ("task_title", VarKind::Caller),
                ("due_at", VarKind::Caller),
                ("notes", VarKind::Caller),
                ("output_language", VarKind::Caller),
            ],
            // `timezone` is `Caller`, not `Optional`: the module's date-resolution rule is
            // meaningless without it, and a default would silently resolve "12 Nov" in the
            // wrong zone — which is the one mistake a syllabus import cannot survive.
            // `semester_context` is the one thing the module can do without: a document
            // that never names its semester is normal, and the default says so.
            ModuleId::SyllabusV1 => &[
                ("timezone", VarKind::Caller),
                ("semester_context", VarKind::Optional(NO_SEMESTER_CONTEXT)),
                ("output_language", VarKind::Optional(SOURCE_LANGUAGE)),
            ],
            // `topics_json` is machine JSON the server built from the student's sources, not
            // user text, so it is `Caller` and deliberately not in `UNTRUSTED_VARIABLES`.
            // `goal_title` is the student's own words and is.
            ModuleId::PlannerEffortV1 => &[
                ("goal_title", VarKind::Caller),
                ("target_date", VarKind::Caller),
                ("topics_json", VarKind::Caller),
                ("output_language", VarKind::Optional(SOURCE_LANGUAGE)),
            ],
        }
    }

    /// The `base.v1.md` sections this module keeps.
    pub fn base_sections(self) -> &'static [&'static str] {
        match self {
            ModuleId::FlashcardsV1
            | ModuleId::QuizV1
            | ModuleId::ReportOutlineV1
            | ModuleId::ReportSectionV1
            | ModuleId::CheatsheetV1
            | ModuleId::QuickCaptureV1
            | ModuleId::TaskBreakdownV1
            | ModuleId::SyllabusV1
            | ModuleId::PlannerEffortV1 => TASK_BASE_SECTIONS,
        }
    }
}
