//! Provider capability traits.
//!
//! Every external dependency the domain needs (embeddings, object storage,
//! time, mail) is a trait here so the server can swap a real backend for a
//! deterministic double and `crates/core` stays free of IO, web and clocks.
//! The traits are defined once, in core, so parallel phases cannot drift into
//! two slightly different `Embedder`s.

pub mod clock;
pub mod embedder;
pub mod llm;
pub mod mailer;
pub mod search;
pub mod storage;
pub mod testing;

pub use clock::Clock;
pub use embedder::{EmbedError, Embedder};
pub use llm::{
    ChatMsg, ChatRequest, ChatResponse, ChatStream, LlmError, LlmProvider, Role, StreamEvent,
    ToolCall, ToolSpec, Usage,
};
pub use mailer::{Email, MailError, Mailer};
pub use search::{
    FetchError, FetchedPage, PageFetcher, SearchError, SearchHit, SearchProvider, SourceKind,
};
pub use storage::{Storage, StorageError};
