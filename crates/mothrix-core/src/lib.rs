// SPDX-License-Identifier: MPL-2.0

//! Everything of mothrix that is not the terminal: the Matrix backend of each account
//! ([`matrix`]) and several of them at once ([`accounts`]), the plain data
//! a UI shows ([`model`]), and what reads messages for showing them
//! (Markdown, code, polls, emoji, spelling). Nothing here draws or reads
//! keys; the terminal UI is the crate `mothrix-tui`.

pub mod accounts;
pub mod art;
pub mod client_state;
pub mod codeblock;
pub mod config;
pub mod convert;
pub mod emoji;
pub mod folders;
pub mod forward;
pub mod inline;
pub mod languagetool;
pub mod matrix;
pub mod message;
pub mod model;
pub mod prefs;
pub mod preview;
pub mod push;
pub mod room_watch;
pub mod search;
pub mod sections;
pub mod sidebar;
pub mod spell;
pub mod stable_polls;
pub mod theme;
#[cfg(windows)]
pub mod toast;
pub mod update;
pub mod verify;
