//! Reusable terminal primitives. ratatui ships none of these.

pub mod highlight;
pub mod input;
pub mod table;

pub use highlight::Highlight;
pub use input::{Input, InputAction};
pub use table::{EdgeBehaviour, KeyValueTable, TableAction};
pub mod checkbox;
pub(crate) mod clipboard;
pub mod editor;
pub mod fuzzy;
pub mod popup;
pub mod select;
pub mod syntax;
pub mod tree;

pub use checkbox::{Checkbox, CheckboxAction};
pub use editor::{Editor, EditorAction};
pub use fuzzy::Match;
pub use popup::{Popup, PopupAction, PopupItem};
pub use select::{Select, SelectAction};
pub use syntax::{Highlighter, Language};
pub use tree::{NodeId, Tree, TreeAction, TreeNode};
