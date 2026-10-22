//! Shared logic for the Stardust tray app.
//!
//! The desktop endpoints described in [`api`] are proposed. ORBIT does not
//! serve them yet. This crate prepares outbound posts and does not open a
//! network connection.

#![forbid(unsafe_code)]

pub mod api;
pub mod check;
pub mod config;
pub mod copy;
pub mod integrations;
pub mod mcp;
pub mod outbound;
pub mod pet;
pub mod policy;
pub mod position;
pub mod schedule;
pub mod text;
pub mod token;
