//! Shared logic for the Stardust tray app.
//!
//! The desktop endpoints described in [`api`] are proposed. ORBIT does not
//! serve them yet, and this crate does not open a network connection.

#![forbid(unsafe_code)]

pub mod api;
pub mod check;
pub mod config;
pub mod copy;
pub mod policy;
pub mod position;
pub mod schedule;
pub mod text;
pub mod token;
