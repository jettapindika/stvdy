//! Prompt assembly for chat: gathers the values `base.v2.md` declares and hands
//! them to the core composer.
//!
//! Everything here is *plumbing*: the prompt text lives in `prompts/base.v2.md`
//! and the composition rules live in `crates/core/src/prompts/`. A prompt string
//! written in this file would be the thing the roadmap forbids, because it could
//! not be versioned or reviewed as prose.

use stvdy_core::prompts::{ChatVars, Composed, PromptChunk, PromptWebHit, compose_chat};

use crate::auth::extract::AuthUser;
use crate::chat::retrieve::RetrievedChunk;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// The assistant's name, as the prompt's `{{assistant_name}}`.
const ASSISTANT_NAME: &str = "Stvdy";
const APP_NAME: &str = "Stvdy";

/// Builds the chat prompt for one request.
///
/// `now` comes from the injected `Clock`, never `Utc::now`, so a test can pin the
/// date a relative phrase resolves against.
// Each argument is an independent fact about the turn (who, which workspace,
// what was retrieved, and the three toggles); bundling them would only hide
// that. The codebase allows the same on `auth::routes::user_view`.
#[allow(clippy::too_many_arguments)]
pub fn compose(
    state: &AppState,
    user: &AuthUser,
    workspace_name: &str,
    chunks: &[RetrievedChunk],
    web: &[PromptWebHit],
    use_web: bool,
    sources_only: bool,
    study_mode: bool,
) -> AppResult<Composed> {
    let vars = base_vars(
        state,
        &user.email,
        &user.timezone,
        user.effective_plan(),
        study_mode,
        use_web,
        sources_only,
        workspace_name,
    );

    // The label is the index plus one, which is what the composer writes into the
    // prompt and what the citation resolver maps back. The ids ride along so
    // `Composed.labels` can resolve a label without a second lookup.
    let prompt_chunks: Vec<PromptChunk> = chunks
        .iter()
        .enumerate()
        .map(|(index, chunk)| PromptChunk {
            label: index as u32 + 1,
            chunk_id: chunk.chunk_id,
            source_id: chunk.source_id,
            source_title: chunk.source_title.clone(),
            page: chunk.page,
            content: chunk.content.clone(),
        })
        .collect();

    compose_chat(&vars, &prompt_chunks, web)
        .map_err(|error| AppError::Internal(anyhow::anyhow!("composing the prompt: {error}")))
}

/// The base-prompt variables for one request.
///
/// Extracted because a task prompt is composed on the same trimmed base, and two
/// builders would eventually disagree about `{{platform}}` or `{{plan_name}}`. The
/// disagreement would show up as a prompt describing the wrong app, which is not
/// something a test would catch.
///
/// It takes the individual values rather than an `AuthUser` because the generation
/// job has no `AuthUser`: it has a row. Fabricating one (with a device id and a
/// session id it does not have) would be a lie in the type system.
// Same as `compose`: every parameter is a distinct fact about the request, and
// the generation jobs call this with the row they loaded.
#[allow(clippy::too_many_arguments)]
pub fn base_vars(
    state: &AppState,
    email: &str,
    timezone: &str,
    plan: stvdy_core::config::PlanId,
    study_mode: bool,
    use_web: bool,
    sources_only: bool,
    workspace_name: &str,
) -> ChatVars {
    let plan_config = state.config.plans.get(plan);
    let now_local = state
        .clock
        .now()
        .with_timezone(&timezone.parse().unwrap_or(chrono_tz::UTC));

    ChatVars {
        assistant_name: ASSISTANT_NAME.to_string(),
        app_name: APP_NAME.to_string(),
        now: now_local.format("%Y-%m-%d %H:%M %Z").to_string(),
        timezone: timezone.to_string(),
        // The prompt tells the model to keep phone replies short, so this has to be
        // the real platform rather than a guess.
        platform: "unknown".to_string(),
        plan_name: plan_config.name.clone(),
        study_mode: if study_mode { "on" } else { "off" }.to_string(),
        study_hours_summary: "not set".to_string(),
        use_web: if use_web { "on" } else { "off" }.to_string(),
        sources_only: if sources_only { "on" } else { "off" }.to_string(),
        // The billing policy decides whether an upgrade hint is allowed at all,
        // and it is per platform. `web` is the permissive default here because
        // the request has not yet carried its platform; the Phase 3+ client sends
        // it, and `billing_policy.validate()` is what stops a mobile build from
        // being permissive in the first place.
        allow_upgrade_hint: state
            .config
            .billing_policy
            .purchase_allowed(stvdy_core::config::Platform::Web),
        user_name: email.to_string(),
        workspace_name: workspace_name.to_string(),
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn the_prompt_is_never_built_from_a_string_literal_here() {
        // The prompt lives in prompts/base.v2.md and is compiled in by the core
        // registry. A prompt sentence in this file would break the roadmap rule
        // ("prompts live in versioned modules, never inline") in the one place it
        // is easiest to break.
        //
        // The scan stops at the test module: the assertions below name the
        // literals they forbid, so a whole-file scan would find them here and
        // fail on itself.
        let whole = include_str!("prompt.rs");
        let source = match whole.find("#[cfg(test)]") {
            Some(index) => &whole[..index],
            None => whole,
        };

        for forbidden in [
            "<grounding_and_citations>",
            "<security_untrusted_content>",
            "Reply in the language",
            "Cite every factual claim",
        ] {
            assert!(
                !source.contains(forbidden),
                "prompt text must come from the registry, not from this module: found {forbidden:?}"
            );
        }
    }
}
