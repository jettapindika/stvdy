//! The Stvdy server: Axum routes, auth, sync, job worker and integrations.

pub mod auth;
pub mod chat;
pub mod chunks;
pub mod config;
pub mod credits;
pub mod db;
pub mod devices;
pub mod error;
pub mod flashcards;
pub mod integrations;
pub mod jobs;
pub mod limits;
pub mod quizzes;
pub mod ratelimit;
pub mod routes;
pub mod sources;
pub mod state;
pub mod telemetry;
pub mod testing;
pub mod util;
pub mod workspaces;
