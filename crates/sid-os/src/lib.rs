//! `sid-os` - Linux implementations of sid-core's OS-integration seams: editor launch
//! (`$VISUAL`/`$EDITOR`, terminal vs GUI editors, `xdg-open` fallback) and desktop
//! notifications (`org.freedesktop.Notifications`). Other platforms are empty slots.

pub mod editor;
pub mod notify;

pub use editor::{EditorEnv, LaunchPlan, LinuxEditorLauncher, plan_launch};
pub use notify::DbusNotifier;
