//! Sync protocol DTOs shared by the server and the native client shell.
//!
//! The types here are the wire contract: the server serialises them and the
//! Tauri shell deserialises them, so a change to this crate is a protocol
//! change and needs a matching change on both sides.

pub mod proto;
