//! Result ranking and dedupe.
//!
//! Pure and deterministic: the composite caches a ranked set and two runs of the
//! same query must return the same order, or the golden tests and any paging built
//! on top of them are flaky. Dedupe runs before the sort, because the order of two
//! copies of one paper is not a ranking question.

use std::cmp::Ordering;

use chrono::{DateTime, Utc};

use crate::search::normalize::normalize_url;
use crate::traits::search::{SearchHit, SourceKind};

/// A hit plus the keys dedupe and sorting need, so each is computed once.
struct Ranked {
    /// The tier that produced the hit, as a rank: 0 for the first provider seen in
    /// the input, 1 for the next, and so on. Hits from one tier share a rank, so
    /// within a tier the order is decided by score and then URL rather than by
    /// input position, which is what makes the output total.
    tier: usize,
    url_key: String,
    doi_key: Option<String>,
    hit: SearchHit,
}

/// Dedupe then order. Deterministic: the same input always gives the same output.
///
/// Order: Academic before Web, then a hit with a DOI before one without, then
/// `published_at` descending with `None` LAST (a naive sort puts `None` first,
/// which is the bug this exists to avoid), then the tier order the hits arrived
/// in, then the provider's own score descending, then the normalised URL so the
/// order is total.
pub fn rank(hits: Vec<SearchHit>) -> Vec<SearchHit> {
    let mut ranked: Vec<Ranked> = Vec::new();
    let mut tiers: Vec<String> = Vec::new();

    for hit in hits {
        // A hit whose URL does not normalise cannot be cited usefully, so it is
        // dropped here rather than ranked and then filtered by every caller.
        let Some(url_key) = normalize_url(&hit.url) else {
            continue;
        };

        // A provider's rank is fixed by where its first hit arrived, so the whole
        // tier sorts as a block.
        let tier = match tiers.iter().position(|seen| seen == &hit.provider) {
            Some(at) => at,
            None => {
                tiers.push(hit.provider.clone());
                tiers.len() - 1
            }
        };

        let doi_key = hit.doi.as_deref().and_then(normalize_doi);
        ranked.push(Ranked {
            tier,
            url_key,
            doi_key,
            hit,
        });
    }

    // DOI first: the same paper at two hosts is one work, and the DOI is the only
    // identifier that survives the host changing.
    let mut by_doi: Vec<Ranked> = Vec::new();
    for entry in ranked {
        let duplicate = entry.doi_key.as_deref().and_then(|doi| {
            by_doi
                .iter()
                .position(|kept| kept.doi_key.as_deref() == Some(doi))
        });
        match duplicate {
            Some(at) => {
                if preferred(&entry, &by_doi[at]) {
                    by_doi[at] = entry;
                }
            }
            None => by_doi.push(entry),
        }
    }

    // Then by normalised URL: the same page carrying different tracking parameters
    // is one result.
    let mut unique: Vec<Ranked> = Vec::new();
    for entry in by_doi {
        match unique.iter().position(|kept| kept.url_key == entry.url_key) {
            Some(at) => {
                if preferred(&entry, &unique[at]) {
                    unique[at] = entry;
                }
            }
            None => unique.push(entry),
        }
    }

    unique.sort_by(compare);
    unique.into_iter().map(|entry| entry.hit).collect()
}

/// Whether `candidate` should replace `incumbent` as the survivor of a duplicate
/// group.
///
/// Academic beats Web because the academic record *is* the work and a web page is
/// a copy of it. Among copies of the same kind the longer title wins: a provider
/// that truncated a title at 80 characters produced a worse record than one that
/// kept it whole.
fn preferred(candidate: &Ranked, incumbent: &Ranked) -> bool {
    match (
        candidate.hit.source_kind == SourceKind::Academic,
        incumbent.hit.source_kind == SourceKind::Academic,
    ) {
        (true, false) => true,
        (false, true) => false,
        _ => candidate.hit.title.chars().count() > incumbent.hit.title.chars().count(),
    }
}

fn compare(a: &Ranked, b: &Ranked) -> Ordering {
    kind_rank(a.hit.source_kind)
        .cmp(&kind_rank(b.hit.source_kind))
        .then_with(|| b.doi_key.is_some().cmp(&a.doi_key.is_some()))
        .then_with(|| compare_published(a.hit.published_at, b.hit.published_at))
        .then_with(|| a.tier.cmp(&b.tier))
        .then_with(|| compare_score(a.hit.score, b.hit.score))
        .then_with(|| a.url_key.cmp(&b.url_key))
}

fn kind_rank(kind: SourceKind) -> u8 {
    match kind {
        SourceKind::Academic => 0,
        SourceKind::Web => 1,
    }
}

/// Newest first, and a missing date LAST. Writing this as `a.cmp(&b)` on the
/// `Option` would put `None` first, which reads as "undated is the most recent
/// thing we know about" and is the single most likely bug in this module.
fn compare_published(a: Option<DateTime<Utc>>, b: Option<DateTime<Utc>>) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) => b.cmp(&a),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

/// Higher score first, with a missing score last: a provider that reported no
/// score is not evidence of a top result.
fn compare_score(a: Option<f32>, b: Option<f32>) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) => b.partial_cmp(&a).unwrap_or(Ordering::Equal),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

/// A DOI reduced to the part that identifies the work, so `10.1/AbC` and
/// `https://doi.org/10.1/abc` are one paper. Case-insensitive because a DOI is,
/// and the resolver prefix is not part of the identifier.
fn normalize_doi(raw: &str) -> Option<String> {
    let raw = raw.trim().to_ascii_lowercase();
    let stripped = raw
        .strip_prefix("https://doi.org/")
        .or_else(|| raw.strip_prefix("http://doi.org/"))
        .or_else(|| raw.strip_prefix("doi:"))
        .unwrap_or(raw.as_str());
    if stripped.is_empty() {
        None
    } else {
        Some(stripped.to_string())
    }
}
