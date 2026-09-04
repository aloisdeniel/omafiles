//! omafiles — a keyboard-first file explorer for Omarchy.
//!
//! The model lives here, deliberately free of gpui, so directory reading,
//! sorting and navigation are testable without a display. `main.rs` is the
//! gpui shell around it.
//!
//! `preview` is the one exception, and a shallow one: it names gpui's `Image`
//! because that type is a format tag beside a byte buffer with no window or
//! context behind it, and inventing a parallel type to copy into would be
//! ceremony. Everything in it still runs on a background thread and its tests
//! open no display.

pub mod actions;
pub mod config;
pub mod entry;
pub mod fileops;
pub mod git;
pub mod grep;
pub mod imageops;
pub mod keymap;
pub mod listing;
pub mod navigation;
pub mod network;
pub mod places;
pub mod preview;
pub mod recent;
pub mod search;
pub mod server;
pub mod session;
pub mod views;
