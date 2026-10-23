//! AriadUsage macOS menu-bar app.
//!
//! Layout, geometry and presentation live in safe, platform-independent
//! modules so they can be tested anywhere. The `appkit` module is the only
//! place that talks to Objective-C and the only place `unsafe` is allowed.

pub mod card;
pub mod card_layout;
#[cfg(unix)]
pub mod engine_client;
pub mod format;
pub mod geometry;
pub mod highlight_style;
pub mod icon;
pub mod pace_text;
pub mod presentation;
pub mod progress_bar;

#[cfg(target_os = "macos")]
pub mod appkit;
