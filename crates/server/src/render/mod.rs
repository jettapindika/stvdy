//! Document rendering: the last step of the cheatsheet pipeline.
//!
//! Everything before this is pure (`stvdy_core::render`), and this module is the only place
//! a browser is involved. Keeping the boundary sharp is what lets the whole renderer be
//! unit-tested without Chromium and what confines the phase's native dependency to one
//! file.

pub mod chrome;
