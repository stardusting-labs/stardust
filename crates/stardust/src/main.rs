//! Stardust tray app.
//!
//! The process keeps no window while the character is hidden and no speech
//! bubble is showing. A bubble is created for a check result and dropped
//! when it closes.

#![forbid(unsafe_code)]

mod actions;
mod app;
mod autostart;
mod cli;
mod deliver;
mod font;
mod icon;
mod inbox;
mod integrations_cmd;
mod keychain;
mod live;
mod notify;
mod oauth;
mod signin;
mod sprites;
mod tray;
mod window;

use std::process::ExitCode;

fn main() -> ExitCode {
    match app::run(std::env::args().skip(1)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("stardust: {err}");
            ExitCode::FAILURE
        }
    }
}
