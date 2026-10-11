//! The ingestion formats Phase 1 never built: DOCX, PPTX, URL and `web`.
//!
//! The DOCX and PPTX fixtures are **built in this file with `zip`**, not committed as
//! binary blobs. That is the point: the test states what the format *is* — a zip
//! whose `word/document.xml` holds `<w:t>` runs, or whose `ppt/slides/slideN.xml`
//! holds `<a:t>` runs — so a reader can check the extractor against the format
//! rather than against a fixture whose provenance nobody can see.
//!
//! The zip guards are asserted through the *source's own error*, because that is
//! what the client shows: a bomb and a zip-slip entry both end as a `failed` source
//! with a sentence explaining why, and neither leaves chunks behind.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::io::Write;
use std::sync::{Arc, Mutex};

use axum::http::StatusCode;
use sqlx::PgPool;
use uuid::Uuid;

use common::{json, post_json, signed_up};
use stvdy_core::traits::search::{FetchError, FetchedPage, PageFetcher};
use stvdy_server::integrations::fetch::{ChainFetcher, ReqwestFetcher};
use stvdy_server::testing::TestState;

// --- fixtures ---------------------------------------------------------------

/// One multipart field: its name, its plain-text value, and an optional file.
type Field<'a> = (&'a str, &'a str, Option<(&'a str, &'a [u8])>);

/// A multipart body with an explicit content type for the file part.
///
/// The shared helper labels every file `application/pdf`, which would make a DOCX
/// upload claim the wrong media type and let a bug in the mime guard pass.
fn multipart_typed(fields: &[Field<'_>]) -> (String, Vec<u8>) {
    let boundary = "----stvdy-format-boundary";
    let mut body = Vec::new();
    for (name, value, file) in fields {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        match file {
            Some((filename, bytes)) => {
                body.extend_from_slice(
                    format!(
                        "Content-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"\r\n\
                         Content-Type: application/octet-stream\r\n\r\n"
                    )
                    .as_bytes(),
                );
                body.extend_from_slice(bytes);
            }
            None => {
                body.extend_from_slice(
                    format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
                );
                body.extend_from_slice(value.as_bytes());
            }
        }
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

/// A zip built in memory from `(member name, contents)` pairs.
fn zip_bytes(entries: &[(&str, &str)]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, content) in entries {
        writer
            .start_file(
                *name,
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated),
            )
            .expect("a member starts");
        writer
            .write_all(content.as_bytes())
            .expect("a member writes");
    }
    writer.finish().expect("the archive finishes").into_inner()
}

/// The OOXML wrapper a real Word file puts around its body, so the fixture is the
/// format rather than a fragment that happens to parse.
fn docx(body: &str) -> Vec<u8> {
    let document = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>{body}</w:body>
</w:document>"#
    );
    zip_bytes(&[
        ("[Content_Types].xml", "<Types/>"),
        ("word/document.xml", &document),
    ])
}

/// One `<w:p>` with a single `<w:t>` run.
fn docx_paragraph(text: &str) -> String {
    format!("<w:p><w:r><w:t>{text}</w:t></w:r></w:p>")
}

/// A PPTX whose slides are `(slide number, text)`.
fn pptx(slides: &[(i32, &str)]) -> Vec<u8> {
    let mut members: Vec<(String, String)> =
        vec![("[Content_Types].xml".to_string(), "<Types/>".to_string())];
    for (number, text) in slides {
        let xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
       xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
  <p:cSld><p:spTree><p:sp><p:txBody>
    <a:p><a:r><a:t>{text}</a:t></a:r></a:p>
  </p:txBody></p:sp></p:spTree></p:cSld>
</p:sld>"#
        );
        members.push((format!("ppt/slides/slide{number}.xml"), xml));
    }
    let refs: Vec<(&str, &str)> = members
        .iter()
        .map(|(name, content)| (name.as_str(), content.as_str()))
        .collect();
    zip_bytes(&refs)
}

/// A zip bomb: one member whose decompressed size is past the guard.
///
/// Deflated zeros, so the archive on the wire is tiny and the *declared*
/// uncompressed size is what the guard has to catch. That is the real shape of the
/// attack: the bomb is small to send and large to read.
fn zip_bomb() -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    writer
        .start_file(
            "word/document.xml",
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated),
        )
        .expect("the member starts");
    let chunk = vec![b'0'; 1024 * 1024];
    // 65 MiB, one past the 64 MiB guard.
    for _ in 0..65 {
        writer.write_all(&chunk).expect("the member writes");
    }
    writer.finish().expect("the archive finishes").into_inner()
}

// --- the fetch double -------------------------------------------------------

/// What a stub answers with.
enum Reply {
    Page {
        title: String,
        text: String,
        final_url: String,
    },
    Refused(String),
}

/// A fetcher that answers from a table and records what it was asked for.
///
/// The recorded URL is the assertion that matters for the wiring: the URL source
/// path must go through the chain in `AppState`, not through a second, unguarded
/// `reqwest` call that a test with a stub could not see.
struct StubFetcher {
    reply: Reply,
    asked: Mutex<Vec<String>>,
}

impl StubFetcher {
    fn page(text: &str, title: &str, final_url: &str) -> Arc<Self> {
        Arc::new(Self {
            reply: Reply::Page {
                title: title.to_string(),
                text: text.to_string(),
                final_url: final_url.to_string(),
            },
            asked: Mutex::new(Vec::new()),
        })
    }

    fn refused(reason: &str) -> Arc<Self> {
        Arc::new(Self {
            reply: Reply::Refused(reason.to_string()),
            asked: Mutex::new(Vec::new()),
        })
    }

    fn asked(&self) -> Vec<String> {
        self.asked
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

#[async_trait::async_trait]
impl PageFetcher for StubFetcher {
    fn name(&self) -> &'static str {
        "stub"
    }

    async fn fetch(&self, url: &str) -> Result<FetchedPage, FetchError> {
        self.asked
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(url.to_string());

        match &self.reply {
            Reply::Page {
                title,
                text,
                final_url,
            } => Ok(FetchedPage {
                final_url: final_url.clone(),
                title: title.clone(),
                text: text.clone(),
                fetched_at: chrono::Utc::now(),
            }),
            Reply::Refused(reason) => Err(FetchError::Refused(reason.clone())),
        }
    }
}

// --- helpers ----------------------------------------------------------------

async fn router(harness: &TestState) -> axum::Router {
    harness.seed_plans().await.expect("the plan seed");
    stvdy_server::routes::router(harness.state.clone()).expect("the router")
}

async fn create_workspace(app: &axum::Router, token: &str, name: &str) -> String {
    let (status, body) = json(
        app,
        "POST",
        "/workspaces",
        token,
        serde_json::json!({ "name": name }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["workspace"]["id"]
        .as_str()
        .expect("a workspace id")
        .to_string()
}

async fn upload(
    app: &axum::Router,
    token: &str,
    workspace_id: &str,
    content_type: &str,
    body: Vec<u8>,
) -> (StatusCode, serde_json::Value) {
    post_json(
        app,
        "POST",
        &format!("/workspaces/{workspace_id}/sources"),
        token,
        Some(content_type),
        body,
    )
    .await
}

async fn source_status(app: &axum::Router, token: &str, source_id: &str) -> serde_json::Value {
    let (status, body) = json(
        app,
        "GET",
        &format!("/sources/{source_id}/status"),
        token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

/// The chunk texts of one source, in order.
async fn chunks(harness: &TestState, source_id: &str) -> Vec<(Option<i32>, String)> {
    sqlx::query_as("select page, content from chunks where source_id = $1 order by idx")
        .bind(Uuid::parse_str(source_id).expect("a source id"))
        .fetch_all(&harness.state.pool)
        .await
        .expect("reading the chunks")
}

// --- the tests --------------------------------------------------------------

/// A DOCX is zip + OOXML: `word/document.xml`, every `<w:t>` run in order, no page
/// numbers because a DOCX has none.
#[sqlx::test(migrations = "../../migrations")]
async fn a_docx_source_ingests_its_paragraph_text(pool: PgPool) {
    let harness = TestState::new(pool);
    let app = router(&harness).await;
    let token = signed_up(&app, &harness, "docx@ugm.ac.id").await;
    let workspace = create_workspace(&app, &token, "Cell biology").await;

    let document = docx(&format!(
        "{}{}",
        docx_paragraph("The mitochondrion is the powerhouse of the cell."),
        docx_paragraph("Ribosomes assemble proteins from amino acids."),
    ));
    let (content_type, body) = multipart_typed(&[
        ("kind", "docx", None),
        ("title", "Week 3 handout", None),
        ("file", "week3.docx", Some(("week3.docx", &document))),
    ]);

    let (status, created) = upload(&app, &token, &workspace, &content_type, body).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let source_id = created["source"]["id"]
        .as_str()
        .expect("a source id")
        .to_string();
    assert_eq!(created["source"]["kind"], "docx");
    assert_eq!(
        created["source"]["mime"],
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
    );

    common::drain_jobs(&harness).await;

    let status_body = source_status(&app, &token, &source_id).await;
    assert_eq!(status_body["status"], "ready", "{status_body}");

    let stored = chunks(&harness, &source_id).await;
    assert!(!stored.is_empty(), "the document produced chunks");
    // No page numbers: a DOCX has none, and inventing one would make a citation
    // point at a page the reader cannot find.
    assert!(
        stored.iter().all(|(page, _)| page.is_none()),
        "a DOCX has no pages: {stored:?}"
    );
    let text: String = stored.into_iter().map(|(_, text)| text).collect();
    assert!(
        text.contains("powerhouse of the cell"),
        "the paragraph text must be extracted: {text}"
    );
    assert!(
        text.contains("Ribosomes assemble proteins"),
        "every paragraph must be extracted: {text}"
    );
}

/// A PPTX keeps its slide numbers: `ppt/slides/slideN.xml`, `<a:t>` runs, page = N.
///
/// The citation for slide 7 must say 7, because that is the only way a student
/// finds the slide the answer came from.
#[sqlx::test(migrations = "../../migrations")]
async fn a_pptx_source_keeps_its_slide_numbers(pool: PgPool) {
    let harness = TestState::new(pool);
    let app = router(&harness).await;
    let token = signed_up(&app, &harness, "pptx@ugm.ac.id").await;
    let workspace = create_workspace(&app, &token, "Networks").await;

    // Out of order on purpose: a zip's member order is the writer's choice, so the
    // extractor has to sort rather than trust it.
    let deck = pptx(&[
        (7, "The transport layer multiplexes conversations."),
        (1, "The physical layer moves bits."),
    ]);
    let (content_type, body) = multipart_typed(&[
        ("kind", "pptx", None),
        ("file", "lecture.pptx", Some(("lecture.pptx", &deck))),
    ]);

    let (status, created) = upload(&app, &token, &workspace, &content_type, body).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let source_id = created["source"]["id"]
        .as_str()
        .expect("a source id")
        .to_string();

    common::drain_jobs(&harness).await;
    let status_body = source_status(&app, &token, &source_id).await;
    assert_eq!(status_body["status"], "ready", "{status_body}");

    let stored = chunks(&harness, &source_id).await;
    let slide_seven = stored
        .iter()
        .find(|(_, text)| text.contains("transport layer"))
        .expect("slide 7 produced a chunk");
    assert_eq!(
        slide_seven.0,
        Some(7),
        "the citation for slide 7 must say 7: {stored:?}"
    );

    let slide_one = stored
        .iter()
        .find(|(_, text)| text.contains("physical layer"))
        .expect("slide 1 produced a chunk");
    assert_eq!(slide_one.0, Some(1), "{stored:?}");

    // And the slides come out in order, so the deck reads forwards.
    let ordered: Vec<Option<i32>> = stored.iter().map(|(page, _)| *page).collect();
    let mut sorted = ordered.clone();
    sorted.sort();
    assert_eq!(ordered, sorted, "slides must be in order: {ordered:?}");
}

/// A zip bomb is refused before anything is extracted: the decompressed size is
/// bounded.
///
/// The refusal is asserted through `sources.error`, because that is what the client
/// shows, and through the absence of chunks, because a refusal that still extracted
/// would be a guard that reported rather than guarded.
#[sqlx::test(migrations = "../../migrations")]
async fn a_zip_bomb_is_refused(pool: PgPool) {
    let harness = TestState::new(pool);
    let app = router(&harness).await;
    let token = signed_up(&app, &harness, "bomb@ugm.ac.id").await;
    let workspace = create_workspace(&app, &token, "Bomb").await;

    let bomb = zip_bomb();
    let (content_type, body) = multipart_typed(&[
        ("kind", "docx", None),
        ("file", "bomb.docx", Some(("bomb.docx", &bomb))),
    ]);
    let (status, created) = upload(&app, &token, &workspace, &content_type, body).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let source_id = created["source"]["id"]
        .as_str()
        .expect("a source id")
        .to_string();

    common::drain_jobs(&harness).await;

    let status_body = source_status(&app, &token, &source_id).await;
    assert_eq!(status_body["status"], "failed", "{status_body}");
    let error = status_body["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("expands to"),
        "the refusal must say what tripped: {error}"
    );
    assert!(
        chunks(&harness, &source_id).await.is_empty(),
        "a refused document must produce no chunks"
    );
}

/// A zip-slip entry is refused before anything is extracted: every member path is
/// checked.
#[sqlx::test(migrations = "../../migrations")]
async fn a_zip_slip_entry_is_refused(pool: PgPool) {
    let harness = TestState::new(pool);
    let app = router(&harness).await;
    let token = signed_up(&app, &harness, "slip@ugm.ac.id").await;
    let workspace = create_workspace(&app, &token, "Slip").await;

    // A member whose path climbs out of the archive, alongside a legitimate one so
    // the guard is not passing merely because the document is empty.
    let hostile = zip_bytes(&[
        ("word/document.xml", "<w:document/>"),
        ("../../../../etc/passwd", "root:x:0:0:"),
    ]);
    let (content_type, body) = multipart_typed(&[
        ("kind", "docx", None),
        ("file", "slip.docx", Some(("slip.docx", &hostile))),
    ]);
    let (status, created) = upload(&app, &token, &workspace, &content_type, body).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let source_id = created["source"]["id"]
        .as_str()
        .expect("a source id")
        .to_string();

    common::drain_jobs(&harness).await;

    let status_body = source_status(&app, &token, &source_id).await;
    assert_eq!(status_body["status"], "failed", "{status_body}");
    let error = status_body["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("unsafe member path"),
        "the refusal must say what tripped: {error}"
    );
    assert!(chunks(&harness, &source_id).await.is_empty());
}

/// A URL source fetches through the chain, cleans the page, stores no bytes and
/// counts zero against the storage quota (A157).
#[sqlx::test(migrations = "../../migrations")]
async fn a_url_source_is_fetched_cleaned_and_charges_no_storage(pool: PgPool) {
    let stub = StubFetcher::page(
        "Osmosis is the movement of solvent across a semipermeable membrane.",
        "Osmosis — Wikipedia",
        "https://en.wikipedia.org/wiki/Osmosis",
    );
    let harness =
        TestState::new(pool).with_fetcher(Arc::new(ChainFetcher::new(vec![stub.clone()])));
    let app = router(&harness).await;
    let token = signed_up(&app, &harness, "url@ugm.ac.id").await;
    let workspace = create_workspace(&app, &token, "Biology").await;

    let (content_type, body) = multipart_typed(&[
        ("kind", "url", None),
        ("url", "https://example.test/osmosis", None),
    ]);
    let (status, created) = upload(&app, &token, &workspace, &content_type, body).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let source_id = created["source"]["id"]
        .as_str()
        .expect("a source id")
        .to_string();

    // Nothing is stored at upload: the fetch is the job's, so the request never
    // blocks on someone else's server.
    assert!(
        created["source"]["file_size"].is_null(),
        "a url source stores no bytes: {created}"
    );

    common::drain_jobs(&harness).await;

    let status_body = source_status(&app, &token, &source_id).await;
    assert_eq!(status_body["status"], "ready", "{status_body}");

    // The wiring: the URL went through `AppState.fetcher`, which is what carries the
    // SSRF guard. A second fetch path would be invisible here.
    assert_eq!(
        stub.asked(),
        vec!["https://example.test/osmosis".to_string()],
        "the URL must go through the chain in AppState"
    );

    let stored = chunks(&harness, &source_id).await;
    let text: String = stored.iter().map(|(_, text)| text.clone()).collect();
    assert!(
        text.contains("semipermeable membrane"),
        "the page text must be extracted: {text}"
    );

    // And the storage quota is untouched (A157): `storage_mb` counts
    // `sum(sources.file_size)`, which is null for a URL source.
    let user_id: Uuid = sqlx::query_scalar("select id from users where email = $1")
        .bind("url@ugm.ac.id")
        .fetch_one(&harness.state.pool)
        .await
        .expect("the account");
    assert_eq!(
        stvdy_server::entitlements::storage_bytes(&harness.state.pool, user_id)
            .await
            .expect("the storage total"),
        0,
        "a url source counts zero against the quota"
    );
}

/// A private address is refused by the guard the chain is wired to (A18/A34).
///
/// The chain here holds the **real** `ReqwestFetcher`, and the target is a literal
/// loopback address, so this exercises the production guard rather than a stub: the
/// refusal happens in `resolve_public`, before any connection is attempted.
#[sqlx::test(migrations = "../../migrations")]
async fn a_url_source_refuses_a_private_address(pool: PgPool) {
    let harness = TestState::new(pool).with_fetcher(Arc::new(ChainFetcher::new(vec![Arc::new(
        ReqwestFetcher::new().expect("a reqwest fetcher"),
    )])));
    let app = router(&harness).await;
    let token = signed_up(&app, &harness, "private@ugm.ac.id").await;
    let workspace = create_workspace(&app, &token, "Private").await;

    let (content_type, body) = multipart_typed(&[
        ("kind", "url", None),
        ("url", "http://169.254.169.254/latest/meta-data/", None),
    ]);
    let (status, created) = upload(&app, &token, &workspace, &content_type, body).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let source_id = created["source"]["id"]
        .as_str()
        .expect("a source id")
        .to_string();

    common::drain_jobs(&harness).await;

    let status_body = source_status(&app, &token, &source_id).await;
    assert_eq!(status_body["status"], "failed", "{status_body}");
    let error = status_body["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("refusing to fetch") && error.contains("169.254.169.254"),
        "the refusal must name the address it refused: {error}"
    );
    assert!(chunks(&harness, &source_id).await.is_empty());
}

/// "Save to workspace" creates a `kind = web` source from a search hit — the Phase 3
/// acceptance item that was never wired.
#[sqlx::test(migrations = "../../migrations")]
async fn a_search_hit_can_be_saved_as_a_web_source(pool: PgPool) {
    let stub = StubFetcher::page(
        "Photosynthesis converts light energy into chemical energy.",
        "Photosynthesis",
        "https://example.test/photosynthesis",
    );
    let harness =
        TestState::new(pool).with_fetcher(Arc::new(ChainFetcher::new(vec![stub.clone()])));
    let app = router(&harness).await;
    let token = signed_up(&app, &harness, "save@ugm.ac.id").await;
    let workspace = create_workspace(&app, &token, "Biology").await;

    let (content_type, body) = multipart_typed(&[
        ("kind", "web", None),
        ("url", "https://example.test/photosynthesis", None),
        ("title", "Photosynthesis", None),
    ]);
    let (status, created) = upload(&app, &token, &workspace, &content_type, body).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let source_id = created["source"]["id"]
        .as_str()
        .expect("a source id")
        .to_string();

    assert_eq!(created["source"]["kind"], "web");
    assert_eq!(created["source"]["title"], "Photosynthesis");
    assert_eq!(
        created["source"]["url"], "https://example.test/photosynthesis",
        "a saved hit keeps its URL, so a citation can link back"
    );
    assert!(
        created["source"]["file_size"].as_i64().unwrap_or(0) > 0,
        "a saved page is a snapshot, so its text is stored: {created}"
    );

    common::drain_jobs(&harness).await;
    let status_body = source_status(&app, &token, &source_id).await;
    assert_eq!(status_body["status"], "ready", "{status_body}");

    let stored = chunks(&harness, &source_id).await;
    let text: String = stored.iter().map(|(_, text)| text.clone()).collect();
    assert!(
        text.contains("light energy into chemical energy"),
        "the saved page must become chunks: {text}"
    );

    // The saved source is visible in the workspace list with its URL, which is how
    // the student finds it again.
    let (status, listed) = json(
        &app,
        "GET",
        &format!("/workspaces/{workspace}/sources"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let rows = listed["sources"].as_array().expect("a source list");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["kind"], "web");
    assert_eq!(rows[0]["url"], "https://example.test/photosynthesis");
}

/// A converter that fails still leaves a ready source (A158's "unloadable" half).
///
/// The missing-converter case above covers an unset `SOFFICE_BIN`; this covers the
/// one that is set and wrong, which is the failure a deployment actually hits after
/// a package move. The ingest must survive it, because the document's text comes
/// from the OOXML and not from the converter.
#[sqlx::test(migrations = "../../migrations")]
async fn a_broken_converter_still_ingests_without_a_preview(pool: PgPool) {
    let mut harness = TestState::new(pool);
    let mut state = harness.state.clone();
    std::sync::Arc::make_mut(&mut state.config).soffice_bin =
        Some(std::path::PathBuf::from("/nonexistent/soffice"));
    harness.state = state;

    let app = router(&harness).await;
    let token = signed_up(&app, &harness, "broken@ugm.ac.id").await;
    let workspace = create_workspace(&app, &token, "Handouts").await;

    let document = docx(&docx_paragraph("Mitosis has four phases."));
    let (content_type, body) = multipart_typed(&[
        ("kind", "docx", None),
        ("file", "mitosis.docx", Some(("mitosis.docx", &document))),
    ]);
    let (status, created) = upload(&app, &token, &workspace, &content_type, body).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let source_id = created["source"]["id"]
        .as_str()
        .expect("a source id")
        .to_string();

    common::drain_jobs(&harness).await;

    let status_body = source_status(&app, &token, &source_id).await;
    assert_eq!(status_body["status"], "ready", "{status_body}");
    let text: String = chunks(&harness, &source_id)
        .await
        .into_iter()
        .map(|(_, text)| text)
        .collect();
    assert!(text.contains("four phases"), "{text}");

    let preview: Option<String> =
        sqlx::query_scalar("select preview_key from sources where id = $1")
            .bind(Uuid::parse_str(&source_id).expect("a source id"))
            .fetch_one(&harness.state.pool)
            .await
            .expect("the source row");
    assert!(
        preview.is_none(),
        "a converter that cannot run leaves no key"
    );
}

/// With a converter present, the rendition is stored and the route serves it.
///
/// `SOFFICE_BIN` points at a script that answers the way `soffice
/// --headless --convert-to pdf --outdir <dir> <input>` does, which is what makes
/// this a test of *our* branch — the argument list, the stored object, the
/// `preview_key` write, the signed URL — rather than a test of LibreOffice's layout.
#[cfg(unix)]
#[sqlx::test(migrations = "../../migrations")]
async fn a_present_converter_stores_a_preview_the_route_serves(pool: PgPool) {
    use std::os::unix::fs::PermissionsExt;

    let script = std::env::temp_dir().join(format!("stvdy-fake-soffice-{}", Uuid::now_v7()));
    std::fs::write(
        &script,
        "#!/bin/sh\n\
         outdir=.\n\
         prev=\n\
         for arg in \"$@\"; do\n\
         \x20 if [ \"$prev\" = \"--outdir\" ]; then outdir=\"$arg\"; fi\n\
         \x20 prev=\"$arg\"\n\
         done\n\
         printf '%%PDF-1.4\\n%% a rendition\\n' > \"$outdir/source.pdf\"\n",
    )
    .expect("the stub converter writes");
    let mut permissions = std::fs::metadata(&script)
        .expect("the stub converter's metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&script, permissions).expect("the stub converter is executable");

    let mut harness = TestState::new(pool);
    let mut state = harness.state.clone();
    std::sync::Arc::make_mut(&mut state.config).soffice_bin = Some(script.clone());
    harness.state = state;

    let app = router(&harness).await;
    let token = signed_up(&app, &harness, "preview@ugm.ac.id").await;
    let workspace = create_workspace(&app, &token, "Handouts").await;

    let document = docx(&docx_paragraph("Enzymes are catalysts."));
    let (content_type, body) = multipart_typed(&[
        ("kind", "docx", None),
        ("file", "enzymes.docx", Some(("enzymes.docx", &document))),
    ]);
    let (status, created) = upload(&app, &token, &workspace, &content_type, body).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let source_id = created["source"]["id"]
        .as_str()
        .expect("a source id")
        .to_string();

    common::drain_jobs(&harness).await;

    let status_body = source_status(&app, &token, &source_id).await;
    assert_eq!(status_body["status"], "ready", "{status_body}");

    let preview: Option<String> =
        sqlx::query_scalar("select preview_key from sources where id = $1")
            .bind(Uuid::parse_str(&source_id).expect("a source id"))
            .fetch_one(&harness.state.pool)
            .await
            .expect("the source row");
    let preview_key = preview.expect("the rendition is recorded");

    // The bytes that were stored are the converter's output, not the original.
    let stored = harness
        .state
        .storage
        .get(&preview_key)
        .await
        .expect("the rendition is stored");
    assert!(
        stored.starts_with(b"%PDF-1.4"),
        "the rendition must be the converter's PDF"
    );

    // And the route hands the client a short-lived URL for it.
    let (status, body) = json(
        &app,
        "GET",
        &format!("/sources/{source_id}/preview"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let url = body["url"].as_str().expect("a URL");
    assert!(url.contains("sig="), "{url}");
    assert!(body["expires_at"].is_string(), "{body}");

    let _ = std::fs::remove_file(&script);
}

#[sqlx::test(migrations = "../../migrations")]
async fn saving_a_private_page_is_refused_with_a_reason(pool: PgPool) {
    let stub = StubFetcher::refused(
        "refusing to fetch example.test: it resolves only to non-public addresses",
    );
    let harness =
        TestState::new(pool).with_fetcher(Arc::new(ChainFetcher::new(vec![stub.clone()])));
    let app = router(&harness).await;
    let token = signed_up(&app, &harness, "refused@ugm.ac.id").await;
    let workspace = create_workspace(&app, &token, "Private").await;

    let (content_type, body) = multipart_typed(&[
        ("kind", "web", None),
        ("url", "http://192.168.0.1/admin", None),
    ]);
    let (status, refused) = upload(&app, &token, &workspace, &content_type, body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(refused["code"], "validation");
    assert!(
        refused["message"]
            .as_str()
            .unwrap_or_default()
            .contains("non-public"),
        "{refused}"
    );

    // Nothing was recorded: a refused save must not leave a row the student cannot use.
    let sources: i64 = sqlx::query_scalar("select count(*) from sources")
        .fetch_one(&harness.state.pool)
        .await
        .expect("counting sources");
    assert_eq!(sources, 0);
}

/// Without `SOFFICE_BIN` the source still ingests and the preview is simply absent
/// (A158).
///
/// The point is that the absence is *honest*: the document's text is still there, and
/// the preview route says "no preview" rather than serving an empty file that looks
/// like a scanned document.
#[sqlx::test(migrations = "../../migrations")]
async fn a_missing_soffice_leaves_no_preview_and_still_ingests(pool: PgPool) {
    let harness = TestState::new(pool);
    // The harness's config leaves SOFFICE_BIN unset, which is the deployment this
    // case is about. Assert it, so a future default cannot make this test vacuous.
    assert!(
        harness.state.config.soffice_bin.is_none(),
        "this case is about a deployment with no converter"
    );

    let app = router(&harness).await;
    let token = signed_up(&app, &harness, "nopreview@ugm.ac.id").await;
    let workspace = create_workspace(&app, &token, "Handouts").await;

    let document = docx(&docx_paragraph("Enzymes lower activation energy."));
    let (content_type, body) = multipart_typed(&[
        ("kind", "docx", None),
        ("file", "enzymes.docx", Some(("enzymes.docx", &document))),
    ]);
    let (status, created) = upload(&app, &token, &workspace, &content_type, body).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let source_id = created["source"]["id"]
        .as_str()
        .expect("a source id")
        .to_string();

    common::drain_jobs(&harness).await;

    let status_body = source_status(&app, &token, &source_id).await;
    assert_eq!(
        status_body["status"], "ready",
        "an absent converter must not fail the ingest: {status_body}"
    );
    let text: String = chunks(&harness, &source_id)
        .await
        .into_iter()
        .map(|(_, text)| text)
        .collect();
    assert!(text.contains("activation energy"), "{text}");

    let preview: Option<String> =
        sqlx::query_scalar("select preview_key from sources where id = $1")
            .bind(Uuid::parse_str(&source_id).expect("a source id"))
            .fetch_one(&harness.state.pool)
            .await
            .expect("the source row");
    assert!(preview.is_none(), "no converter means no preview key");

    // And the route says so rather than serving nothing.
    let (status, _) = json(
        &app,
        "GET",
        &format!("/sources/{source_id}/preview"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
