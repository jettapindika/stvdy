//! Calendar: the ICS feed, the bounded expansion of an imported feed, and the conflict
//! decision. Pure — no web, no database, no clock. The server reads rows and calls in; the
//! rules live here so they have tests that do not need a server.

pub mod expand;
pub mod feed;
pub mod policy;
