//! The cheatsheet reply contract (`prompts/cheatsheet.v1.md`).
//!
//! There is deliberately no second definition of the document shape here. The reply the
//! model returns is parsed by [`crate::render::blocks::parse_blocks`] into
//! [`crate::render::blocks::Blocks`], which is the very type the pure renderer takes as
//! input, so the contract that accepts a reply and the contract that renders it are one
//! thing. A copy would be a second thing to keep in step, and the drift it would allow is
//! silent in the worst way: the model call is paid for, the document generates cleanly, and
//! then it renders with a field dropped, or does not render at all, because the two shapes
//! disagreed about one key.
//!
//! The re-exports below are therefore the whole module: a path that names this contract,
//! the types and parser it resolves to, and the bounds that make the reply enforceable.
//! Nothing is wrapped, because there is nothing to add — `deny_unknown_fields`, the
//! per-kind rules, and the caps are all stated once, in the renderer's block model, where
//! the renderer also reads them.

pub use crate::render::blocks::{
    Block, Blocks, Chapter, MAX_BLOCKS_PER_SECTION, MAX_CHAPTERS, MAX_SECTIONS_PER_CHAPTER,
    MAX_TABLE_COLUMNS, Section, parse_blocks,
};
