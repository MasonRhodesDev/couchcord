//! `cc-render` — the gamescope external-overlay renderer boundary.
//!
//! Phase 1 implements the **pure 8-anchor geometry** (criterion 7), which is the
//! part testable without a live X server. The blocking X11 work (override-redirect
//! window, the `GAMESCOPE_EXTERNAL_OVERLAY` atom, display discovery, painting) is
//! Phase 4 — it lands behind the `OverlayRenderer` trait so the geometry here is
//! reused unchanged.

pub mod geometry;
pub mod paint;
pub mod window;

pub use geometry::anchor_rect;
pub use window::X11Overlay;
