//! The half that draws, and the only half that talks to anything.

pub mod application;
pub mod client;
pub mod runner;
pub mod tablet;
pub mod window;

pub use application::RemarkableApplication;
pub use runner::{Outcome, Run};
pub use window::Window;
