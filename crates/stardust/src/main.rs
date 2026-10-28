//! Stardust tray app.
//!
//! The process keeps no window while the character is hidden and no speech
//! bubble is showing. A bubble is created for a check result and dropped
//! when it closes.

#![forbid(unsafe_code)]

mod actions;
mod app;
mod cli;
mod font;
mod icon;
mod inbox;
mod keychain;
mod notify;
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
