//! Platform-independent core of the Tile window manager.
//!
//! [`State`] owns monitors, workspaces and window membership. Every mutation returns a list
//! of [`Effect`]s that the platform layer applies, which keeps all logic testable.

pub mod animation;
pub mod command;
pub mod geometry;
pub mod layout;
pub mod state;

use serde::{Deserialize, Serialize};

pub use command::Command;
pub use geometry::{Direction, Rect};
pub use layout::{LayoutKind, LayoutSettings};
pub use state::{Effect, ManageOptions, MonitorInfo, State, WindowInfo};

/// A top-level window handle value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct WindowId(pub isize);

/// A monitor handle value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MonitorId(pub isize);
