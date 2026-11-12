//! Platform-neutral Tidemark code shared by the Windows app (repository
//! root) and the Linux app. Nothing here may call an OS API beyond `std`:
//! the crate must build and test on Windows and Linux alike.
//!
//! - Models and logic moved from the Windows app (single source; the Windows
//!   crate re-exports them under the old `crate::<module>` paths): `anim`,
//!   `settings_motion`, `layout`, `textinput`, `text_tools`, `keyboard`,
//!   `timer`, `notes`, `clip_history`, `media_artwork`, `config`.
//! - Files: `paths` (Windows `%APPDATA%\win-island`, Linux XDG).
//! - Linux contracts: `shell`, `event`, `service`, plus the agent wire
//!   format `agent_wire` used on both platforms.

pub mod agent_wire;
pub mod anim;
pub mod clip_history;
pub mod config;
pub mod event;
pub mod keyboard;
pub mod layout;
pub mod media_artwork;
pub mod notes;
pub mod paths;
pub mod service;
pub mod settings_motion;
pub mod shell;
pub mod text_tools;
pub mod textinput;
pub mod timer;
