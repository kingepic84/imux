pub mod agentview;
mod compositor;
mod diff;
pub mod dither;
pub mod iterm;
pub mod kitty;
pub mod overlay;
pub mod quad;
pub mod scaled;
pub mod sixel;
pub mod status;
pub mod xterm;

#[cfg(test)]
pub use compositor::compose;
pub use compositor::{BorderLines, Look, PaneStatus, PaneView, PopupView, compose_window_full, compose_window_placed, compose_window_popup, compose_window_with};
pub use diff::{Renderer, sgr};
