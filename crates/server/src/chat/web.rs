//! The web arm of a chat turn: run the search, build the prompt block, and give
//! the citation resolver its `[Wn]` labels.
//!
//! Kept apart from the route because the interesting decisions are here and they
//! are about *degradation*: a query every tier failed is not an error, the turn
//! proceeds without web context, and the labels the model was given are the only
//! ones that can resolve (A17/A3).

use stvdy_core::prompts::PromptWebHit;
use stvdy_core::traits::search::SearchHit;

use crate::integrations::search::composite::SearchOutcome;

/// How much of a snippet goes into the prompt.
///
/// Bounded because a page's snippet can be large and the context has a budget: with
/// eight results, an unbounded snippet would crowd out the user's own sources,
/// which are the material the answer is supposed to be grounded in.
const MAX_SNIPPET_CHARS: usize = 600;

/// The web half of a turn: the hits worth citing, the prompt block, and what the
/// search did.
#[derive(Debug, Clone)]
pub struct WebContext {
    /// Labelled for the prompt, in the order they will be cited.
    pub prompt_hits: Vec<PromptWebHit>,
    /// The same hits in the same order, for the citation resolver's label map.
    pub hits: Vec<SearchHit>,
    /// The tier that answered, or `None` when nothing did.
    pub answered_by: Option<String>,
    /// Tiers consulted and found empty, erroring, or unreachable (A3/A40).
    pub unresponsive: Vec<String>,
    /// Whether the results came from the cache, which decides whether this turn is
    /// charged for a search (A40).
    pub cache_hit: bool,
    /// The SearXNG engines that did not answer, which is the A3 health signal
    /// distinct from an unresponsive *tier*: one engine dying degrades a tier
    /// without failing it.
    pub unresponsive_engines: Vec<String>,
}

impl WebContext {
    /// No web context, because the toggle was off or nothing was found. The turn
    /// still proceeds: a query no tier answered is not an error.
    pub fn empty() -> Self {
        Self {
            prompt_hits: Vec::new(),
            hits: Vec::new(),
            answered_by: None,
            unresponsive: Vec::new(),
            cache_hit: false,
            unresponsive_engines: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.hits.is_empty()
    }
}

/// Turns a search outcome into the prompt block and the label map.
///
/// The labels are assigned here, once, and both the prompt and the resolver read
/// this same vector: a label that exists in the prompt but not in the map (or the
/// reverse) is how a citation ends up pointing at the wrong page.
pub fn build(outcome: SearchOutcome) -> WebContext {
    let mut prompt_hits = Vec::with_capacity(outcome.hits.len());
    let mut hits = Vec::with_capacity(outcome.hits.len());

    for (index, hit) in outcome.hits.into_iter().enumerate() {
        let label = index as u32 + 1;
        prompt_hits.push(PromptWebHit {
            label,
            url: hit.url.clone(),
            title: hit.title.clone(),
            snippet: truncate_snippet(&hit.snippet),
            // The DOI rides along so the model can cite a work by its stable
            // identifier and the citation resolves with it. Dropping it here is how
            // "URLs and DOIs" quietly becomes "URLs".
            doi: hit.doi.clone(),
        });
        hits.push(hit);
    }

    WebContext {
        prompt_hits,
        hits,
        answered_by: outcome.answered_by,
        unresponsive: outcome.unresponsive,
        cache_hit: outcome.cache_hit,
        unresponsive_engines: outcome.unresponsive_engines,
    }
}

/// A snippet shortened on a word boundary, so the prompt never ends mid-word and
/// never carries more than its share of the context.
fn truncate_snippet(snippet: &str) -> String {
    let collapsed = snippet.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= MAX_SNIPPET_CHARS {
        return collapsed;
    }

    let mut out = String::with_capacity(MAX_SNIPPET_CHARS + 1);
    for word in collapsed.split(' ') {
        if out.chars().count() + word.chars().count() + 1 > MAX_SNIPPET_CHARS {
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use stvdy_core::traits::search::SourceKind;

    fn hit(title: &str, snippet: &str) -> SearchHit {
        let mut hit = SearchHit::new(
            title,
            "https://example.org/paper",
            snippet,
            SourceKind::Web,
            "tavily",
        );
        hit.published_at = Some(Utc::now());
        hit
    }

    #[test]
    fn labels_start_at_one_and_are_dense() {
        let outcome = SearchOutcome {
            hits: vec![hit("a", "one"), hit("b", "two"), hit("c", "three")],
            answered_by: Some("tavily".to_string()),
            unresponsive: vec!["searxng".to_string()],
            cache_hit: false,
            unresponsive_engines: vec!["mojeek".to_string()],
        };

        let context = build(outcome);
        let labels: Vec<u32> = context.prompt_hits.iter().map(|h| h.label).collect();
        // A gap would mean the model could cite `[W2]` for the third hit, and the
        // resolver would agree, which is a silent mis-citation.
        assert_eq!(labels, vec![1, 2, 3]);
        assert_eq!(context.hits.len(), 3);
        assert_eq!(context.prompt_hits[0].title, "a");
        assert_eq!(context.unresponsive, vec!["searxng".to_string()]);
        // The engine-level list is separate from the tier-level one: one engine
        // dying degrades a tier without failing it.
        assert_eq!(context.unresponsive_engines, vec!["mojeek".to_string()]);
    }

    #[test]
    fn a_long_snippet_is_cut_on_a_word_boundary() {
        let long = "word ".repeat(400);
        let context = build(SearchOutcome {
            hits: vec![hit("t", &long)],
            answered_by: Some("tavily".to_string()),
            unresponsive: Vec::new(),
            cache_hit: false,
            unresponsive_engines: Vec::new(),
        });

        let snippet = &context.prompt_hits[0].snippet;
        assert!(
            snippet.chars().count() <= MAX_SNIPPET_CHARS + 1,
            "the snippet must be bounded, got {}",
            snippet.chars().count()
        );
        // Cut at a boundary, not mid-word: every part is the word, and the marker
        // is the last character.
        assert!(snippet.ends_with('…'), "{snippet:?}");
        let body = snippet.trim_end_matches('…');
        assert!(
            body.split(' ').all(|part| part == "word"),
            "no partial word may survive: {body:?}"
        );
    }

    #[test]
    fn whitespace_in_a_snippet_is_collapsed() {
        let context = build(SearchOutcome {
            hits: vec![hit("t", "one\n\n  two\t\tthree")],
            answered_by: None,
            unresponsive: Vec::new(),
            cache_hit: false,
            unresponsive_engines: Vec::new(),
        });
        assert_eq!(context.prompt_hits[0].snippet, "one two three");
    }

    #[test]
    fn an_empty_outcome_is_not_an_error() {
        let context = build(SearchOutcome::default());
        assert!(context.is_empty());
        assert!(context.prompt_hits.is_empty());
        assert_eq!(context.answered_by, None);
    }
}
