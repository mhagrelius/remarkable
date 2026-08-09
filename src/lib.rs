//! Remarkable: handwritten notes off a reMarkable tablet, read by a local
//! model and written out as Markdown.
//!
//! Two halves, as in the sibling apps. [`model`] links no GTK and opens no
//! socket — it decides where a tall page can be cut, what to ask about each
//! section, and how to stitch the answers back together, all of which is testable
//! with no display and the GPU asleep. [`ui`] is the only half that knows a
//! window exists, and the only one that talks to llama-server or to the tablet.

pub mod model;
pub mod ui;

pub const APP_ID: &str = "us.hagreli.Remarkable";

/// Where llama-server is, unless told otherwise. The same port llama-tray
/// manages and familiar talks to.
pub const DEFAULT_SERVER: &str = "http://127.0.0.1:8080";
