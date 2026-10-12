//! Pure domain logic: no web, no database, no IO.
//!
//! Everything here must be a pure function of its inputs so it can be unit
//! tested against a fixed clock. Chunking, retrieval, citation resolution,
//! SM-2 replay, the planner, the entitlements engine, the prompt composer and
//! the document renderer.

pub mod agent;
pub mod billing;
pub mod calendar;
pub mod chunk;
pub mod citations;
pub mod config;
pub mod entitlements;
pub mod error;
pub mod generation;
pub mod planner;
pub mod prompts;
pub mod quiz;
pub mod render;
pub mod retrieval;
pub mod search;
pub mod sm2;
pub mod study;
pub mod tasks;
pub mod traits;
