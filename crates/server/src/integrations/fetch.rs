//! Page fetching, behind `stvdy_core::traits::search::PageFetcher`.
//!
//! Three implementations, tried in the order the roadmap fixes (Exa `/contents`,
//! then the `moli` CLI, then `reqwest` plus readability), because each covers what
//! the previous one misses: Exa returns clean text and honours robots upstream,
//! moli renders JavaScript, and plain `reqwest` is the cheapest and handles static
//! pages.
//!
//! **The SSRF rule is the point of this module.** Ingestion accepts an arbitrary
//! URL from a user, so an unguarded fetch is a primitive for reading the server's
//! own network and its cloud metadata endpoint (A18/A34). Every path therefore
//! refuses a non-http(s) scheme and a target that resolves into a private range,
//! and the `reqwest` path re-checks every redirect hop, because a public URL that
//! redirects to `169.254.169.254` is the standard bypass.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use chrono::Utc;
use stvdy_core::traits::search::{FetchError, FetchedPage, PageFetcher};

/// How many redirects are followed before giving up. Bounded because an endless
/// redirect chain is a denial-of-service on our own worker.
const MAX_REDIRECTS: usize = 5;

/// The largest response body either guarded path will read.
///
/// The same 8 MiB the ICS expansion refuses above (`calendar::expand::MAX_BODY_BYTES`), and
/// deliberately enforced *here* rather than only there: the expansion's check runs after the
/// body is a `String` in memory, so without a bound at the socket a hostile URL can make the
/// worker hold hundreds of megabytes before anything looks at the size. A page fetch gets the
/// same bound, because "save this page" is not a reason to buffer an unbounded body either.
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

/// The validators a conditional GET sends (A239).
///
/// A struct rather than the `Option<(&str, &str)>` pair a signature would suggest, because the
/// two are independent: an ICS server may send an `ETag` and no `Last-Modified`, and a pair
/// would force an empty-string sentinel for the missing one — which is a header whose value is
/// `""`, not an absent header. Both `None` is an unconditional request.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Validators {
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

/// A body fetched as text through the guard, with no readability pass (A233).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuardedText {
    /// The URL actually served, after any redirects. A redirect target is re-validated, so
    /// this is also the URL the guard last approved.
    pub final_url: String,
    /// The body. Empty when `not_modified`.
    pub body: String,
    /// The response's `ETag`, for the next conditional GET.
    pub etag: Option<String>,
    /// The response's `Last-Modified`, for the next conditional GET.
    pub last_modified: Option<String>,
    /// The server answered `304 Not Modified`: the caller's validators still match, so there
    /// is nothing to parse and nothing to write. A field rather than an empty-body check,
    /// because an ICS document that is genuinely empty and a 304 are different facts and a
    /// sync that confused them would either re-import nothing forever or skip a real change.
    pub not_modified: bool,
}

/// A user-supplied URL fetched as text, through the **same** guard as [`ReqwestFetcher`] and
/// without readability (A233): an ICS body is not HTML, and `extract_readable` would hand back
/// a tag-stripped document — a silently empty import, with no error anywhere.
///
/// `If-None-Match`/`If-Modified-Since` are sent when the caller has validators, and a `304` is
/// `Ok` with `not_modified` set and an empty body.
pub async fn fetch_guarded_text(
    url: &str,
    accept: &str,
    validators: Option<&Validators>,
) -> Result<GuardedText, FetchError> {
    let response = guarded_get(url, accept, validators).await?;

    if response.status == reqwest::StatusCode::NOT_MODIFIED {
        return Ok(GuardedText {
            final_url: response.final_url,
            body: String::new(),
            etag: response.etag,
            last_modified: response.last_modified,
            not_modified: true,
        });
    }
    if !response.status.is_success() {
        return Err(FetchError::Provider(format!(
            "the server answered {}",
            response.status
        )));
    }

    Ok(GuardedText {
        final_url: response.final_url,
        body: response.body,
        etag: response.etag,
        last_modified: response.last_modified,
        not_modified: false,
    })
}

/// One guarded GET: the scheme and host check, the address check, a client pinned to a
/// checked address, and a bounded redirect loop with every hop re-validated.
///
/// Extracted from `ReqwestFetcher::fetch` so there is **one** redirect loop and not two
/// (A233). The guard is the thing that must not be duplicated — a second copy is a second
/// place for the pinned-address step or the per-hop re-check to be forgotten, and the failure
/// is a request to `169.254.169.254` that looks like an ordinary fetch.
///
/// Returns the response without interpreting it: a `304` is not a success and not an error, and
/// only the caller knows whether a body is HTML to extract or text to parse.
async fn guarded_get(
    url: &str,
    accept: &str,
    validators: Option<&Validators>,
) -> Result<GuardedResponse, FetchError> {
    let mut current = parse_target(url)?;

    for _ in 0..=MAX_REDIRECTS {
        let host = current
            .host_str()
            .ok_or_else(|| FetchError::Invalid("the URL has no host".to_string()))?
            .to_string();
        let port = current.port_or_known_default().unwrap_or(443);
        let allowed = resolve_public(&host, port).await?;

        // Pin the connection to an address that was just checked, so a second
        // DNS answer arriving between the check and the connect cannot point
        // somewhere else. `resolve` is on the client builder, so this is a
        // per-request client; both callers are rare operations, so losing
        // connection pooling across requests costs nothing that matters.
        let pinned = allowed[0];
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent("Stvdy/0.1 (+https://stvdy.app)")
            .resolve(&host, pinned)
            .build()
            .map_err(|error| FetchError::Provider(error.to_string()))?;

        let mut request = client.get(current.clone()).header("accept", accept);
        if let Some(validators) = validators {
            // An empty validator is an absent one, never a header with an empty value: a
            // server reads `If-None-Match: ` as a malformed request, not as "no opinion".
            if let Some(etag) = validators.etag.as_deref().filter(|value| !value.is_empty()) {
                request = request.header(reqwest::header::IF_NONE_MATCH, etag);
            }
            if let Some(modified) = validators
                .last_modified
                .as_deref()
                .filter(|value| !value.is_empty())
            {
                request = request.header(reqwest::header::IF_MODIFIED_SINCE, modified);
            }
        }

        let response = request
            .send()
            .await
            .map_err(|error| FetchError::Provider(error.to_string()))?;

        if response.status().is_redirection() {
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| {
                    FetchError::Provider("a redirect with no Location header".to_string())
                })?;

            // Relative redirects are resolved against the current URL, then
            // re-validated exactly like the original: `parse_target` checks the
            // scheme and the credentials, and the next iteration re-resolves and
            // re-checks the addresses. Without the `parse_target` call here a
            // redirect to a non-http scheme would slip past the guard and fail
            // later inside `send()` as a provider error, which reads like an
            // outage rather than a refusal.
            current = parse_target(
                current
                    .join(location)
                    .map_err(|error| FetchError::Invalid(error.to_string()))?
                    .as_str(),
            )?;
            continue;
        }

        let final_url = response.url().to_string();
        let etag = header_value(&response, reqwest::header::ETAG);
        let last_modified = header_value(&response, reqwest::header::LAST_MODIFIED);
        let status = response.status();
        // A `304` carries no body, and `bytes_stream` on it yields nothing, so this is the
        // same path either way.
        let body = read_bounded(response).await?;

        return Ok(GuardedResponse {
            final_url,
            status,
            etag,
            last_modified,
            body,
        });
    }

    Err(FetchError::Provider(format!(
        "more than {MAX_REDIRECTS} redirects"
    )))
}

/// The response, without its body having been interpreted.
struct GuardedResponse {
    final_url: String,
    status: reqwest::StatusCode,
    etag: Option<String>,
    last_modified: Option<String>,
    body: String,
}

/// One header, as a `String`, or `None` when it is absent or not valid UTF-8.
///
/// A non-UTF-8 `ETag` is not a reason to fail a fetch — it is a validator we cannot echo, and
/// the next poll is then unconditional, which is the safe direction.
fn header_value(response: &reqwest::Response, name: reqwest::header::HeaderName) -> Option<String> {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

/// Reads a response body, refusing one larger than [`MAX_RESPONSE_BYTES`].
///
/// Streamed rather than `response.text()`, because `text()` buffers whatever the peer sends and
/// only then can the size be looked at. A chunked response has no `Content-Length` to check
/// first, so the only place the bound can be enforced is while reading.
///
/// The decode is `String::from_utf8_lossy`, which is exactly what `reqwest`'s own `text()`
/// does in this build: the workspace enables `reqwest` without its `charset` feature, so
/// `text()` is the `from_utf8_lossy` branch and no behaviour is lost by reading the bytes here.
async fn read_bounded(response: reqwest::Response) -> Result<String, FetchError> {
    use futures::StreamExt;

    let mut collected: Vec<u8> = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| FetchError::Provider(error.to_string()))?;
        if collected.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err(FetchError::Refused(format!(
                "refusing a response larger than {MAX_RESPONSE_BYTES} bytes"
            )));
        }
        collected.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8_lossy(&collected).into_owned())
}

/// Whether an address is one this server refuses to connect to.
///
/// Explicit rather than `!is_global()`, which is unstable, and deliberately
/// generous: it is better to refuse a public address that happens to fall in a
/// reserved range than to allow one that reaches a cloud metadata service.
pub fn is_public_ip(addr: IpAddr) -> bool {
    match addr {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => is_public_v6(v6),
    }
}

fn is_public_v4(addr: Ipv4Addr) -> bool {
    // `is_private` covers 10/8, 172.16/12 and 192.168/16.
    if addr.is_private()
        || addr.is_loopback()
        || addr.is_link_local()
        || addr.is_unspecified()
        || addr.is_multicast()
        || addr.is_broadcast()
    {
        return false;
    }

    let octets = addr.octets();
    // Ranges the standard predicates do not cover, each of which is a real
    // target: carrier-grade NAT (100.64/10), IETF protocol assignments
    // (192.0.0/24), benchmarking (198.18/15), reserved (240/4), and 192.0.2/24
    // plus friends (TEST-NET).
    match octets {
        [100, second, ..] if (64..128).contains(&second) => false,
        [192, 0, 0, _] => false,
        [192, 0, 2, _] => false,
        [198, 18 | 19, _, _] => false,
        [198, 51, 100, _] => false,
        [203, 0, 113, _] => false,
        [first, ..] if first >= 240 => false,
        _ => true,
    }
}

fn is_public_v6(addr: Ipv6Addr) -> bool {
    // An IPv4-mapped address (`::ffff:169.254.169.254`) reaches an IPv4 target, so
    // it has to be judged as one. Missing this is the classic way a v6 check is
    // bypassed.
    if let Some(v4) = addr.to_ipv4_mapped() {
        return is_public_v4(v4);
    }

    if addr.is_loopback() || addr.is_unspecified() || addr.is_multicast() {
        return false;
    }

    let segments = addr.segments();
    // Unique local addresses (fc00::/7) and link-local (fe80::/10).
    if segments[0] & 0xfe00 == 0xfc00 {
        return false;
    }
    if segments[0] & 0xffc0 == 0xfe80 {
        return false;
    }
    // 6to4 and Teredo tunnel an IPv4 target, so they are judged by the v4 rules
    // where the embedded address is extractable.
    if segments[0] == 0x2002 {
        let embedded = Ipv4Addr::new(
            (segments[1] >> 8) as u8,
            segments[1] as u8,
            (segments[2] >> 8) as u8,
            segments[2] as u8,
        );
        return is_public_v4(embedded);
    }

    true
}

/// The syntactic and literal-address check, with the reason.
///
/// [`parse_target`] covers the scheme, the host and the credentials; this adds the two things
/// that can be decided **without DNS**: a literal address in a private or reserved range, and
/// the loopback names every resolver maps to the loopback interface. It is what `is_citable_url`
/// decides with, and what `POST /calendar/ics` refuses a URL with (A246) — the user gets
/// "that is not a URL we can fetch" at save time instead of a connection that silently never
/// syncs.
///
/// A hostname that resolves to a private address still passes here, and that residual is
/// deliberate rather than an oversight: resolving it would be a DNS lookup in a request
/// handler, and DNS can change between the save and the fetch anyway. The fetch-time guard
/// (which does resolve, and pins the checked address) is the protection; this is the courtesy.
///
/// `Url::host()` rather than `host_str()`: the string form keeps an IPv6 literal's brackets
/// (`[::1]`), so parsing it as an `IpAddr` fails and the address check silently does not run.
/// The suite caught exactly that.
pub fn check_fetchable_literal(raw: &str) -> Result<url::Url, FetchError> {
    let parsed = parse_target(raw)?;
    match parsed.host() {
        Some(url::Host::Ipv4(addr)) if !is_public_ip(IpAddr::V4(addr)) => Err(FetchError::Refused(
            format!("refusing to fetch {addr}: it is not a public address"),
        )),
        Some(url::Host::Ipv6(addr)) if !is_public_ip(IpAddr::V6(addr)) => Err(FetchError::Refused(
            format!("refusing to fetch {addr}: it is not a public address"),
        )),
        Some(url::Host::Domain(host)) if is_loopback_name(host) => Err(FetchError::Refused(
            format!("refusing to fetch {host}: it resolves to the loopback interface"),
        )),
        _ => Ok(parsed),
    }
}

/// Whether a search result's URL may be shown to the user as a citation.
///
/// The citation path never goes through the fetcher: a hit's URL is written into
/// `Citation::Web` and rendered as a link the user clicks, which
/// `platform.openExternal` then opens in their browser. So a hostile or
/// compromised search tier could return `http://169.254.169.254/latest/meta-data/`
/// or `http://127.0.0.1:8099/` and have it offered as a citable source. Filtering
/// at the tier boundary is what makes the property true for every consumer at once,
/// instead of each one having to remember.
///
/// This is a *syntactic* check and a *literal* address check: a hostname that
/// resolves to a private address still passes here, because resolving every hit
/// would be a DNS lookup per result. That residual is recorded rather than
/// claimed solved, and the fetch path (which does resolve) is the one that has to
/// be airtight.
pub fn is_citable_url(raw: &str) -> bool {
    check_fetchable_literal(raw).is_ok()
}

/// Names that resolve to loopback without a DNS lookup.
///
/// `localhost` and its dotted form, plus the `*.localhost` convention that RFC 6761
/// reserves and every modern resolver maps to the loopback interface.
fn is_loopback_name(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    host == "localhost" || host.ends_with(".localhost")
}

/// The scheme and host check, before any DNS work.
pub fn parse_target(raw: &str) -> Result<url::Url, FetchError> {
    let parsed = url::Url::parse(raw).map_err(|error| FetchError::Invalid(error.to_string()))?;

    match parsed.scheme() {
        "http" | "https" => {}
        other => {
            return Err(FetchError::Refused(format!(
                "refusing to fetch a {other} URL: only http and https are allowed"
            )));
        }
    }

    // Belt and braces: `url` already rejects an empty authority for http and
    // https, so this cannot fire today. It stays because the guard is the thing
    // that must not be wrong, and a future switch to a lenient parser would
    // otherwise silently hand an empty name to the resolver.
    match parsed.host_str() {
        None => return Err(FetchError::Invalid("the URL has no host".to_string())),
        Some(host) if host.trim().is_empty() => {
            return Err(FetchError::Invalid("the URL has an empty host".to_string()));
        }
        Some(_) => {}
    }

    // A URL carrying credentials would send them to whatever answers.
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(FetchError::Refused(
            "refusing a URL with embedded credentials".to_string(),
        ));
    }

    Ok(parsed)
}

/// Resolves a host and returns only its public addresses, or refuses.
///
/// Returning the addresses rather than a boolean is deliberate: the caller pins the
/// connection to one of them, which is what closes the DNS-rebinding window
/// between the check and the connect.
pub async fn resolve_public(host: &str, port: u16) -> Result<Vec<SocketAddr>, FetchError> {
    // A literal address skips DNS, and must still be checked.
    if let Ok(literal) = host.parse::<IpAddr>() {
        if is_public_ip(literal) {
            return Ok(vec![SocketAddr::new(literal, port)]);
        }
        return Err(FetchError::Refused(format!(
            "refusing to fetch {host}: it is not a public address"
        )));
    }

    let resolved: Vec<SocketAddr> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|error| FetchError::Provider(format!("resolving {host}: {error}")))?
        .collect();

    if resolved.is_empty() {
        return Err(FetchError::Provider(format!("{host} resolved to nothing")));
    }

    let public: Vec<SocketAddr> = resolved
        .into_iter()
        .filter(|addr| is_public_ip(addr.ip()))
        .collect();

    if public.is_empty() {
        // Every address was private. Refusing rather than trying the next one is
        // the safe direction: a hostname that resolves to both a public and a
        // private address is an attack, not a misconfiguration.
        return Err(FetchError::Refused(format!(
            "refusing to fetch {host}: it resolves only to non-public addresses"
        )));
    }

    Ok(public)
}

/// The `reqwest` path, with readability extraction and a guarded redirect chain.
pub struct ReqwestFetcher;

impl ReqwestFetcher {
    pub fn new() -> anyhow::Result<Self> {
        Ok(Self)
    }
}

#[async_trait::async_trait]
impl PageFetcher for ReqwestFetcher {
    fn name(&self) -> &'static str {
        "reqwest"
    }

    async fn fetch(&self, url: &str) -> Result<FetchedPage, FetchError> {
        // The guard, the redirect loop and the body bound live in `guarded_get`, shared with
        // `fetch_guarded_text` (A233). What is *not* shared is this line: a web page is HTML
        // and readability is what turns it into the text a source is made of, while an ICS
        // body must be handed over byte for byte.
        let response = guarded_get(url, "*/*", None).await?;

        if !response.status.is_success() {
            return Err(FetchError::Provider(format!(
                "the server answered {}",
                response.status
            )));
        }

        let (title, text) = extract_readable(&response.body, &response.final_url);
        Ok(FetchedPage {
            final_url: response.final_url,
            title,
            text,
            fetched_at: Utc::now(),
        })
    }
}

/// Readability extraction, with a fallback that still yields text.
///
/// A page the algorithm scores as unreadable (a table-heavy index, a PDF served as
/// HTML) must not produce an empty source: the caller asked to save this page, and
/// an empty document is worse than a crude one.
fn extract_readable(html: &str, base: &str) -> (String, String) {
    // Readability needs a base URL to resolve relative links. The caller passes the
    // URL the page was actually fetched from, which is always parseable because it
    // came from `reqwest`; the fallback exists so a pathological value degrades to
    // the tag stripper rather than panicking in a worker.
    let parsed = url::Url::parse(base).ok();

    if let Some(url) = &parsed
        && let Ok(product) = readability::extractor::extract(&mut html.as_bytes(), url)
        && !product.text.trim().is_empty()
    {
        return (product.title, product.text);
    }

    let title = html_title(html).unwrap_or_else(|| base.to_string());
    (title, strip_tags(html))
}

fn html_title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let start = lower.find("<title")?;
    let open_end = lower[start..].find('>')? + start + 1;
    let end = lower[open_end..].find("</title>")? + open_end;
    let title = html[open_end..end].trim().to_string();
    (!title.is_empty()).then_some(title)
}

/// A crude tag stripper for the fallback path.
fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for character in html.chars() {
        match character {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                // A tag boundary is a word boundary, so a space keeps words apart.
                out.push(' ');
            }
            other if !in_tag => out.push(other),
            _ => {}
        }
    }

    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The `moli` CLI, for pages that need JavaScript.
///
/// `--block-private-networks` is mandatory (A18/A34): without it this is an SSRF
/// primitive, and it is the flag's whole purpose. `moli`'s own help notes that a
/// proxy-resolved target bypasses that client-side policy, which is why this
/// implementation never passes `--http-proxy`.
pub struct MoliFetcher {
    binary: String,
}

impl MoliFetcher {
    pub fn new() -> Self {
        Self {
            binary: "moli".to_string(),
        }
    }
}

impl Default for MoliFetcher {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl PageFetcher for MoliFetcher {
    fn name(&self) -> &'static str {
        "moli"
    }

    async fn fetch(&self, url: &str) -> Result<FetchedPage, FetchError> {
        // The URL is checked here too, even though moli checks it as well: a
        // missing binary must not silently become an unchecked fetch, and the
        // scheme check costs nothing.
        let parsed = parse_target(url)?;

        let output = tokio::process::Command::new(&self.binary)
            .args([
                "fetch",
                "--dump",
                "markdown",
                "--timeout",
                "15000",
                "--block-private-networks",
                parsed.as_str(),
            ])
            .output()
            .await
            .map_err(|error| FetchError::Provider(format!("running {}: {error}", self.binary)))?;

        if !output.status.success() {
            // moli writes the reason to stderr, and it is the difference between
            // "the page refused us" and "the flag rejected the target".
            let reason = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(FetchError::Provider(if reason.is_empty() {
                format!("{} exited with {}", self.binary, output.status)
            } else {
                reason
            }));
        }

        let text = String::from_utf8_lossy(&output.stdout).to_string();
        if text.trim().is_empty() {
            return Err(FetchError::Provider(
                "the page produced no text".to_string(),
            ));
        }

        Ok(FetchedPage {
            final_url: parsed.to_string(),
            title: parsed.host_str().unwrap_or("Fetched page").to_string(),
            text,
            fetched_at: Utc::now(),
        })
    }
}

/// Exa `/contents`, which returns clean text and honours robots upstream.
pub struct ExaContentsFetcher {
    client: reqwest::Client,
    api_key: String,
}

impl ExaContentsFetcher {
    pub fn new(api_key: String) -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|error| anyhow::anyhow!("building the Exa client: {error}"))?;
        Ok(Self { client, api_key })
    }
}

#[async_trait::async_trait]
impl PageFetcher for ExaContentsFetcher {
    fn name(&self) -> &'static str {
        "exa_contents"
    }

    async fn fetch(&self, url: &str) -> Result<FetchedPage, FetchError> {
        // Checked before handing the URL to Exa. Exa does the fetching, so this
        // does not protect our network, but it does stop us using a paid service as
        // a proxy for a target we would refuse to touch ourselves, and it keeps
        // every path's behaviour identical for the caller.
        let parsed = parse_target(url)?;

        let response = self
            .client
            .post("https://api.exa.ai/contents")
            .header("x-api-key", &self.api_key)
            .json(&serde_json::json!({ "urls": [parsed.as_str()], "text": true }))
            .send()
            .await
            .map_err(|error| FetchError::Provider(error.to_string()))?;

        if !response.status().is_success() {
            return Err(FetchError::Provider(format!(
                "Exa answered {}",
                response.status()
            )));
        }

        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|error| FetchError::Provider(error.to_string()))?;

        let first = body
            .get("results")
            .and_then(|results| results.get(0))
            .ok_or_else(|| FetchError::Provider("Exa returned no result".to_string()))?;

        let text = first
            .get("text")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string();
        if text.trim().is_empty() {
            return Err(FetchError::Provider(
                "Exa returned no text for this page".to_string(),
            ));
        }

        Ok(FetchedPage {
            final_url: first
                .get("url")
                .and_then(|value| value.as_str())
                .unwrap_or(parsed.as_str())
                .to_string(),
            title: first
                .get("title")
                .and_then(|value| value.as_str())
                .unwrap_or_else(|| parsed.host_str().unwrap_or("Fetched page"))
                .to_string(),
            text,
            fetched_at: Utc::now(),
        })
    }
}

/// Tries each fetcher in order and returns the first that works.
///
/// A failure is not fatal until every fetcher has failed: Exa may be unconfigured,
/// moli may not be installed, and plain `reqwest` may be defeated by JavaScript,
/// which is exactly why all three exist.
pub struct ChainFetcher {
    fetchers: Vec<std::sync::Arc<dyn PageFetcher>>,
}

impl ChainFetcher {
    pub fn new(fetchers: Vec<std::sync::Arc<dyn PageFetcher>>) -> Self {
        Self { fetchers }
    }

    /// Builds the chain from configuration: Exa when a key is present, moli when
    /// the binary is on the path, and `reqwest` always, since it needs nothing.
    pub fn from_config(exa_api_key: Option<String>) -> anyhow::Result<Self> {
        let mut fetchers: Vec<std::sync::Arc<dyn PageFetcher>> = Vec::new();

        if let Some(key) = exa_api_key {
            fetchers.push(std::sync::Arc::new(ExaContentsFetcher::new(key)?));
        }

        let moli = MoliFetcher::new();
        if moli_is_installed() {
            fetchers.push(std::sync::Arc::new(moli));
        } else {
            tracing::info!(
                "the moli CLI is not on PATH; JavaScript-rendered pages will fall through to reqwest"
            );
        }

        fetchers.push(std::sync::Arc::new(ReqwestFetcher::new()?));
        Ok(Self::new(fetchers))
    }

    /// The fetchers actually in the chain, for the boot log and the tests.
    pub fn names(&self) -> Vec<&'static str> {
        self.fetchers.iter().map(|fetcher| fetcher.name()).collect()
    }
}

fn moli_is_installed() -> bool {
    std::process::Command::new("moli")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[async_trait::async_trait]
impl PageFetcher for ChainFetcher {
    fn name(&self) -> &'static str {
        "chain"
    }

    async fn fetch(&self, url: &str) -> Result<FetchedPage, FetchError> {
        // A refusal is not something the next fetcher can fix, so it stops the
        // chain. Without this, a `javascript:` URL would be refused by Exa and
        // then handed to moli and reqwest.
        let _ = parse_target(url)?;

        let mut last: Option<FetchError> = None;
        for fetcher in &self.fetchers {
            match fetcher.fetch(url).await {
                Ok(page) => return Ok(page),
                Err(FetchError::Refused(reason)) => return Err(FetchError::Refused(reason)),
                Err(error) => {
                    tracing::debug!(fetcher = fetcher.name(), %error, "fetch attempt failed");
                    last = Some(error);
                }
            }
        }

        Err(last
            .unwrap_or_else(|| FetchError::Provider("no page fetcher is configured".to_string())))
    }
}
