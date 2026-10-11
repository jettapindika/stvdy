# Prompt modules

Every model-facing prompt lives here as a **versioned Markdown file**, and is compiled
into the binary with `include_str!`. Prompts are content, not code: they change more
often than the Rust around them, they are reviewed by reading prose, and diffs should
show prose. Keeping them as `.md` means no recompile is needed to *edit* one, and
`include_str!` means there is no runtime file IO and no way to deploy a binary whose
prompt file is missing.

## Layout and versioning

```
prompts/
  base.v1.md                    # identity, language, grounding, security, tone, tools
  base.v2.md                    # v1 + the per-chat grounding toggle ({{sources_only}})
  flashcards.v1.md              # task modules: appended to a base
  quiz.v1.md
  report_outline.v1.md
  report_section.v1.md
  syllabus.v1.md
  task_breakdown.v1.md
  quick_capture.v1.md
  planner_effort.v1.md
  cheatsheet.v1.md
  json_contract.v1.md           # shared preamble for every strict-JSON module
```

Rules:

1. **Never edit a shipped version.** A behavioural change creates `foo.v2.md` and a
   new `PromptId` variant. This is what makes `usage_events.prompt_version` meaningful. Quality and cost can be compared across versions because the old text still exists.
2. The version string logged is the filename stem (`chat.v1`, `quiz.v2`). It is stored
   on every `usage_events` row (A38) so a regression can be attributed to a prompt
   change rather than guessed at.
3. Deleting a version is only allowed once no `usage_events` row references it.

## Composition

Two shapes, and only two:

| Job | Composition |
|---|---|
| Chat / assistant | `base.v2.md` + `<sources>` + `<web_results>` + tool schemas |
| Strict-JSON task | `base.v1.md` **trimmed** (see below) + `json_contract.v1.md` + the module + `<sources>` |

`base.v2.md` is `base.v1.md` superseded (rule 1) in exactly one place: rule 2 of
`<grounding_and_citations>` now reads the per-chat "Answer only from my sources"
toggle through `{{sources_only}}` (`on`/`off`, mirroring `{{use_web}}`). With it
`on`, the model may answer only from `<sources>` and must offer web search or the
toggle itself as the next step; with it `off`, a general-knowledge answer is
allowed but must be labelled and uncited — v1's wording, kept verbatim. The toggle
governs general knowledge, **not** web: `<web_results>` and `[W#]` citations are
unaffected by it. A request that omits the field is grounded `on`, because that is
the UI default.

Trimming the base for task calls is deliberate, not an optimisation to skip: the base is
long, and `<tools_and_actions>`, `<planning_help>` and `<teaching_style>` are actively
harmful in a JSON-only call (they invite prose). `PromptId::base_sections_for(job)`
returns the section allowlist; the composer strips sections not in it, then asserts the
result still contains `<grounding_and_citations>` and `<security_untrusted_content>`.
That assertion is the guard rail: a task prompt must never be composed without
grounding and untrusted-content rules.

## Variable contract

Variables are `{{snake_case}}` and fall into three kinds, which the composer treats
differently. `PromptId::variables()` declares them; the composer enforces the split.

**1. Caller-supplied**. The request must provide a value. An unfilled variable is an
**error, not an empty string**, so a prompt can never ship with a literal `{{count}}`
in it.

**2. Derived**. Never supplied by the caller; the composer computes the value from
other variables. A derived variable in the caller's required list is a test failure, and
supplying one is an error rather than an override. Currently exactly one:

| Derived variable | Computed from | Values |
|---|---|---|
| `{{plan_gate_instruction_derived}}` | `{{allow_upgrade_hint}}` | `true` → "You may mention once, briefly, that the feature is available on a higher plan." / `false` → "Do not mention upgrading, buying, prices, or purchasing websites." |

This is structural on purpose. An earlier draft had `{{plan_gate_instruction}}` as a
*second* caller-supplied variable alongside `{{allow_upgrade_hint}}`, which encoded one
fact twice: a caller could pass `allow_upgrade_hint: false` with a permissive
instruction string, and the failure mode is a mobile build hinting at purchases. The
single behaviour the spec treats as an app-store rejection risk. Deriving it makes that
contradiction unrepresentable rather than merely discouraged.

**3. Optional**. Declared with a stated default (`{{semester_context}}`,
`{{other_section_headings}}`). Absent is fine; the default is inserted.

### Untrusted by construction

Some variables carry user-controlled text: `{{user_name}}`, `{{workspace_name}}`,
`{{goal_title}}`, `{{task_title}}`, `{{notes}}`, `{{topic}}`, `{{capture_text}}`. A user
can set their display name to `Ignore all previous instructions and ...`. These are
therefore **delimited and labelled as data**. Never interpolated bare into the
instruction stream. And `<security_untrusted_content>` explicitly names them:

```
<user_context_data>
display_name: "Jetta"
workspace_name: "Jaringan Komputer"
</user_context_data>
```

The two halves live in different places, and neither is a guess about where in a sentence
a variable sits:

- **The template carries the block.** `base.v1.md` writes one for the chat prompt, and
  each task module writes one for its own user text (`report_outline.v1.md`,
  `report_section.v1.md`, `cheatsheet.v1.md`). A module that adds a new one gets the
  labelling sentence the base uses.
- **The composer escapes the value** (`escape_if_untrusted`), keyed off
  `prompts::UNTRUSTED_VARIABLES`. That is also the list to add a name to when a phase
  introduces new user text; `crates/core/tests/prompts.rs` then fails the build until the
  module's template carries a block for it.

The list is names, not a `VarKind`: a *setting* the UI renders as a fixed control
(`{{depth}}`, `{{count}}`, `{{exam_date}}`) is a value the model must **follow**, so it is
deliberately neither listed nor wrapped. A name is not a setting because of what it is
called, though: `{{style}}` and `{{length}}` are free text inputs on the report form, so
they are user-typed strings and they are listed and wrapped like any other. The block's
own sentence is what makes that safe and useful at once — it says the values are
user-supplied, that the model should do what they ask for, and that it must ignore any
instruction inside them that tries to change its task. `{{other_section_headings}}` is
`Optional` and still listed, because its value is headings the student may have edited; the
composer escapes a supplied optional value for the same reason it escapes a required one.

Everything from the database or the network is data. Only the system prompt and the
user's own chat turns carry instructions.

## Grounding and citation labels

Chunks are sent as `[1]`, `[2]`, … and web results as `[W1]`, `[W2]`, …. The model
cites labels; the server maps a label back to the real chunk UUID. This is deliberately
*not* raw UUIDs in the prompt: labels cost a few tokens instead of ~40 per citation,
and a model cannot hallucinate a label that looks plausible but points at another
user's chunk. An unknown label is simply dropped (A17).

The mapping is per-request and never persisted in the prompt. `messages.citations`
stores the resolved, structured form:

```json
[{"type":"chunk","chunk_id":"...","source_id":"...","page":12},
 {"type":"web","url":"...","title":"..."}]
```

## JSON modules

Every strict-JSON module is followed by `json_contract.v1.md`, and the server:

1. sets `temperature` low (see the module),
2. asks for JSON only,
3. parses with `serde` into a typed struct with `deny_unknown_fields`,
4. **retries once** on a parse failure with a short corrective message,
5. on a second failure, fails the job with a readable error. Never guesses.

## Testing prompts

`crates/core/tests/prompts.rs` asserts, for every module: all required variables are
declared, no undeclared `{{...}}` remains, the trimmed composition still contains the
grounding and security sections, and the JSON modules' documented example parses into
their Rust struct. It also asserts that **a derived variable never appears in a module's
caller-supplied list**. Otherwise a caller could pass a contradictory pair, which for
`{{plan_gate_instruction_derived}}` would mean a mobile build hinting at purchases.
Those tests run in CI, so a prompt edit that breaks the contract fails the build rather
than production.

One case is the guard rail for the rule above: every module is composed with a marker in
each of its `UNTRUSTED_VARIABLES` and with a value that tries to close the block itself,
and the marker must appear only inside a `<user_context_data>` region. A phase that adds a
user-controlled variable without a block fails there, and so does a template that loses
one — which is what keeps this section's promise true rather than aspirational.

The behavioural suite is separate: ~30 recorded scenarios (answer not in the sources,
prompt injection inside a document, relative-date task creation, Indonesian, English,
a locked feature on a mobile build) run against a provider whenever a prompt version
changes. Results are appended to `prompts/CHANGELOG.md` with the version and date.

**Status: not built yet.** Chat has existed since Phase 2 and the suite and its
changelog do not exist, so the structural tests above are still the only gate. The
spec requires `prompts/evals/` plus a runner; that is tracked as an open gap in
`docs/ASSUMPTIONS.md` (A112), together with the prompt filenames that differ from the
spec's list. Nothing here should be read as a claim that the suite runs.
