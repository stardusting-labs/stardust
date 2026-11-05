//! Shared logic for the Stardust tray app.
//!
//! Device sign-in follows the ORBIT account routes. [`http`] calls the
//! desktop status and alerts routes. Webhook helpers still do not open a
//! socket; only the ORBIT client does.

#![forbid(unsafe_code)]

pub mod a11y;
pub mod api;
pub mod autostart;
pub mod check;
pub mod config;
pub mod connect;
pub mod copy;
pub mod dnd;
pub mod http;
pub mod i18n;
pub mod integrations;
pub mod mcp;
pub mod outbound;
pub mod pet;
pub mod policy;
pub mod position;
pub mod route;
pub mod schedule;
pub mod session;
pub mod settings;
pub mod signin_window;
pub mod sink;
#[cfg(test)]
mod testserver;
pub mod text;
pub mod token;
