//! Cheatsheets: typed JSON blocks, a pure render to HTML, and a Chromium print.
//!
//! The pipeline is three steps and they are separated on purpose:
//!
//! 1. `generate_cheatsheet` calls the model once and stores the **validated blocks** (as the
//!    reply text the renderer parses) plus the HTML the pure renderer made of them.
//! 2. `render_cheatsheet_pdf` prints that stored HTML through the pooled Chromium.
//! 3. `POST /cheatsheets/:id/re-render` repeats step 1's render and step 2 from the stored
//!    blocks, with no model call and no credits.
//!
//! A failed print must not fail the generation: the blocks are the document, and a print is
//! retryable and free. That is what makes the two jobs separate rows in the roadmap's job
//! table rather than one.

pub mod generate;
pub mod routes;

pub use routes::routes;
