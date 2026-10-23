//! AriadUsage macOS menu-bar app.
//!
//! Layout, geometry and presentation live in safe, platform-independent
//! modules so they can be tested anywhere. The `appkit` module is the only
//! place that talks to Objective-C and the only place `unsafe` is allowed.

#[cfg(unix)]
pub mod engine_client;
pub mod format;
pub mod icon;
pub mod presentation;

#[cfg(target_os = "macos")]
pub mod appkit;
