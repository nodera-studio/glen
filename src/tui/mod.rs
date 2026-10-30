pub mod ansi;
pub mod keymap;
pub mod layout;
pub mod prompt;
pub mod state;
pub mod theme;
pub mod update;
pub mod view;

#[allow(dead_code)] // Worker entry points are connected by the event-loop slice.
pub(crate) mod sources;
