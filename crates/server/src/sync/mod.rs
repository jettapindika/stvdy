//! The sync protocol's server half.
//!
//! Phase 7 lands the **decision** a push makes about one mutation
//! ([`push::decide`]); Phase 8 lands the transport around it (the advisory lock,
//! the loop, `mutation_log`). The split is deliberate (A150): the decision is a
//! pure function, so `tests/tamper.rs` can drive every `docs/THREAT_MODEL.md` §5
//! "fails" row for the push protocol without a running push route, and Phase 8
//! re-implements none of it.

pub mod push;
