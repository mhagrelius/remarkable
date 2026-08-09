//! The half that links no GTK and opens no socket.
//!
//! The pipeline, in the order a page goes through it:
//!
//! 1. [`raster`] decodes the export and reduces each page to one byte per row.
//! 2. [`sections`] reads that profile and decides where the page can be cut.
//! 3. [`job`] walks the sections, asking [`prompt`] what to send for each.
//! 4. [`wire`] shapes the request and reads the answer.
//! 5. [`text`] tidies each answer and [`merge`] stitches them back together.
//! 6. [`transcript`] writes the result out.
//!
//! Only [`raster`] touches a library that decodes anything, and only [`device`]
//! touches the world. The rest is arithmetic and string handling, which is why
//! most of this crate's tests need neither a display nor a server.

pub mod device;
pub mod document;
pub mod eval;
pub mod job;
pub mod merge;
pub mod prompt;
pub mod raster;
pub mod sections;
pub mod text;
pub mod transcript;
pub mod wire;
