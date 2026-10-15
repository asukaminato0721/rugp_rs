//! Game state and software rendering, independent of the application/window host.
pub mod audio;
pub mod game;
pub mod image_manager;
pub mod render;
pub use game::{Game, RunState};
