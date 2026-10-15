//! rUGP scene serialization and execution-facing instruction model.
pub mod resources;
pub mod scenario;
pub mod vm;
pub use scenario::{Argument, Instruction, Scenario};
