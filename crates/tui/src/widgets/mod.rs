//! Reusable terminal primitives. ratatui ships none of these.

pub mod highlight;
pub mod input;
pub mod table;

pub use highlight::Highlight;
pub use input::{Input, InputAction};
pub use table::{EdgeBehaviour, KeyValueTable, TableAction};
