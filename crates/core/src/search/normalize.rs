//! URL and query normalisation, and the A40 cache key.
//!
//! Pure: no IO, no clock. The canonical URL is a *dedupe key*, not something to
//! show the user, so it may trade a little fidelity for the property that two
//! links to the same document collapse to one string. The query key is what makes
//! the same question asked twice cost one provider call and one credit.

use sha2::{Digest, Sha256};

/// The canonical form of a URL for dedupe and for the cache key.
///
/// `None` for anything that is not http or https, so a `javascript:` or `mailto:`
/// hit is dropped rather than ranked.
pub fn normalize_url(raw: &str) -> Option<String> {
    let raw = raw.trim();

    // The `://` split is what separates a URL from `mailto:a@b.c` or
    // `javascript:alert(1)`, both of which have a colon but no authority, so they
    // fall out here rather than being ranked and then filtered by every caller.
    let (scheme, after) = raw.split_once("://")?;
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return None;
    }

    // A fragment never reaches the server, so it cannot make two URLs different
    // documents and must not split a cache entry.
    let after = match after.split_once('#') {
        Some((before_fragment, _)) => before_fragment,
        None => after,
    };

    let (authority, path_and_query) = match after.find('/') {
        Some(at) => (&after[..at], &after[at..]),
        None => (after, ""),
    };
    // No host means nothing to cite and nothing to connect to, so it is not a URL
    // this system will use.
    if authority.is_empty() {
        return None;
    }
    let authority = authority.to_ascii_lowercase();

    let (path, query) = match path_and_query.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (path_and_query, None),
    };

    // A trailing slash is not part of a resource's identity, but a lone `/` is the
    // root, so it survives: that way `https://e.com` and `https://e.com/` are one
    // key rather than two.
    let path = path.trim_end_matches('/');
    let path = if path.is_empty() { "/" } else { path };

    // The scheme is always written as `https`. http and https at the same host and
    // path are the same document for dedupe, and a scheme difference must not buy
    // the same page twice from a provider or charge the user for it twice.
    let mut canonical = format!("https://{authority}{path}");
    if let Some(query) = query.and_then(kept_query) {
        canonical.push('?');
        canonical.push_str(&query);
    }
    Some(canonical)
}

/// The query string with campaign tags removed, or `None` when nothing meaningful
/// is left so the key does not end in a bare `?`.
fn kept_query(query: &str) -> Option<String> {
    let kept: Vec<&str> = query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .filter(|pair| !is_tracking(pair))
        .collect();
    if kept.is_empty() {
        None
    } else {
        Some(kept.join("&"))
    }
}

/// Whether a `key=value` pair is a campaign tag rather than part of the resource's
/// identity. `ref` and `source` are here because every social platform appends
/// them, and a page reached through three of them is still one page.
fn is_tracking(pair: &str) -> bool {
    let key = match pair.split_once('=') {
        Some((key, _)) => key,
        None => pair,
    };
    let key = key.to_ascii_lowercase();
    key.starts_with("utm_")
        || matches!(
            key.as_str(),
            "fbclid" | "gclid" | "mc_cid" | "mc_eid" | "ref" | "source"
        )
}

/// Lowercased, whitespace-collapsed and trimmed, so "What  Is X?" and "what is x?"
/// are one cache entry.
pub fn normalize_query(raw: &str) -> String {
    raw.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// The A40 cache key: `sha256(normalize_query | provider | max_results)`, hex.
///
/// The provider is in the key because two tiers answer the same query
/// differently, and `max_results` because a wider request must not be served a
/// narrower cached set.
pub fn dedupe_key(query: &str, provider: &str, max_results: u8) -> String {
    let normalized = normalize_query(query);
    let mut hasher = Sha256::new();
    hasher.update(normalized.as_bytes());
    hasher.update(b"|");
    hasher.update(provider.as_bytes());
    hasher.update(b"|");
    hasher.update(max_results.to_string().as_bytes());
    hex::encode(hasher.finalize())
}
