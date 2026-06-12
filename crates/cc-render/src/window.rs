//! The gamescope external-overlay X11 window: a 32-bit ARGB, override-redirect,
//! input-transparent window flagged with `GAMESCOPE_EXTERNAL_OVERLAY` so the
//! compositor draws it on top without it being a focus-stack surface. Presents
//! the tiny-skia `Frame` from `paint`, anchored via the pure geometry.
//!
//! X11 is blocking and non-`Send`-friendly to share, so a real deployment runs
//! this on its own thread (ARCHITECTURE §1.1). The async-trait methods here have
//! synchronous bodies; the composition root is responsible for not blocking the
//! reactor on them.

use crate::geometry::anchor_rect;
use crate::paint;
use cc_core::{Anchor, ConfigSource, OverlayRenderer, RenderError, Scene};
use std::sync::Arc;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    ColormapAlloc, ConnectionExt, CreateWindowAux, EventMask, Gcontext, ImageFormat, PropMode,
    Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

const PAD: u32 = 16;
const MAX_REQUEST_BYTES: usize = 240 * 1024; // conservative per-request image size

pub struct X11Overlay {
    config: Arc<dyn ConfigSource>,
    font: Option<fontdue::Font>,
    anchor: Anchor,
    state: Option<XState>,
}

struct XState {
    conn: RustConnection,
    win: Window,
    gc: Gcontext,
    depth: u8,
    screen: (u16, u16),
    size: (u16, u16),
    mapped: bool,
}

impl X11Overlay {
    pub fn new(config: Arc<dyn ConfigSource>) -> Self {
        let anchor = config.current().anchor;
        X11Overlay { config, font: paint::load_font(), anchor, state: None }
    }
}

#[async_trait::async_trait]
impl OverlayRenderer for X11Overlay {
    async fn realize(&mut self) -> Result<(), RenderError> {
        if self.state.is_some() {
            return Ok(());
        }
        let dpy = discover_gamescope_display()
            .ok_or_else(|| RenderError::new("no gamescope external-overlay display found"))?;
        let (conn, screen_num) =
            x11rb::connect(Some(&dpy)).map_err(|e| RenderError::new(format!("connect {dpy}: {e}")))?;
        let setup = conn.setup();
        let screen = &setup.roots[screen_num];
        let screen_size = (screen.width_in_pixels, screen.height_in_pixels);

        // A 32-bit TrueColor visual for ARGB; fall back to root depth (no alpha).
        let (depth, visual) = find_argb_visual(screen).unwrap_or((screen.root_depth, screen.root_visual));

        let colormap = conn.generate_id().map_err(rerr)?;
        conn.create_colormap(ColormapAlloc::NONE, colormap, screen.root, visual).map_err(rerr)?;

        let win = conn.generate_id().map_err(rerr)?;
        let aux = CreateWindowAux::new()
            .override_redirect(1)
            .background_pixel(0)
            .border_pixel(0)
            .colormap(colormap)
            .event_mask(EventMask::NO_EVENT);
        conn.create_window(depth, win, screen.root, 0, 0, 1, 1, 0, WindowClass::INPUT_OUTPUT, visual, &aux)
            .map_err(rerr)?;

        // Flag as a gamescope external overlay.
        let atom = conn
            .intern_atom(false, b"GAMESCOPE_EXTERNAL_OVERLAY")
            .map_err(rerr)?
            .reply()
            .map_err(rerr)?
            .atom;
        let cardinal = conn.intern_atom(false, b"CARDINAL").map_err(rerr)?.reply().map_err(rerr)?.atom;
        conn.change_property32(PropMode::REPLACE, win, atom, cardinal, &[1]).map_err(rerr)?;

        let gc = conn.generate_id().map_err(rerr)?;
        conn.create_gc(gc, win, &Default::default()).map_err(rerr)?;
        conn.flush().map_err(rerr)?;

        self.state = Some(XState {
            conn,
            win,
            gc,
            depth,
            screen: screen_size,
            size: (1, 1),
            mapped: false,
        });
        Ok(())
    }

    async fn draw(&mut self, scene: &Scene) -> Result<(), RenderError> {
        if self.state.is_none() {
            self.realize().await?;
        }
        let cfg = self.config.current();
        let frame = paint::render(scene, &cfg, self.font.as_ref());
        let anchor = self.anchor;
        let st = self.state.as_mut().ok_or_else(|| RenderError::new("not realized"))?;

        let Some(frame) = frame else {
            // blank → hide
            if st.mapped {
                st.conn.unmap_window(st.win).map_err(rerr)?;
                st.conn.flush().map_err(rerr)?;
                st.mapped = false;
            }
            return Ok(());
        };

        let (w, h) = (frame.width as u16, frame.height as u16);
        let (x, y) = anchor_rect(anchor, (frame.width, frame.height), (st.screen.0 as u32, st.screen.1 as u32), PAD);

        use x11rb::protocol::xproto::ConfigureWindowAux;
        let cfgwin = ConfigureWindowAux::new().x(x).y(y).width(w as u32).height(h as u32);
        st.conn.configure_window(st.win, &cfgwin).map_err(rerr)?;
        st.size = (w, h);

        if !st.mapped {
            st.conn.map_window(st.win).map_err(rerr)?;
            st.mapped = true;
        }

        let bgra = rgba_to_bgra(&frame.pixels);
        put_image_strips(&st.conn, st.win, st.gc, st.depth, w, h, &bgra)?;
        st.conn.flush().map_err(rerr)?;
        Ok(())
    }

    fn set_anchor(&mut self, anchor: Anchor) {
        self.anchor = anchor;
    }
}

fn rerr<E: std::fmt::Display>(e: E) -> RenderError {
    RenderError::new(e.to_string())
}

/// tiny-skia is RGBA; an ARGB X visual on little-endian wants B,G,R,A bytes.
fn rgba_to_bgra(rgba: &[u8]) -> Vec<u8> {
    let mut out = rgba.to_vec();
    for px in out.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    out
}

/// `put_image` in horizontal strips so no single request exceeds the server's
/// max request length.
fn put_image_strips(
    conn: &RustConnection,
    win: Window,
    gc: Gcontext,
    depth: u8,
    w: u16,
    h: u16,
    bgra: &[u8],
) -> Result<(), RenderError> {
    let row_bytes = w as usize * 4;
    if row_bytes == 0 {
        return Ok(());
    }
    let rows_per_strip = (MAX_REQUEST_BYTES / row_bytes).max(1);
    let mut y = 0usize;
    while y < h as usize {
        let strip_rows = rows_per_strip.min(h as usize - y);
        let start = y * row_bytes;
        let end = start + strip_rows * row_bytes;
        conn.put_image(
            ImageFormat::Z_PIXMAP,
            win,
            gc,
            w,
            strip_rows as u16,
            0,
            y as i16,
            0,
            depth,
            &bgra[start..end],
        )
        .map_err(rerr)?;
        y += strip_rows;
    }
    Ok(())
}

/// Find a 32-bit TrueColor visual for ARGB composition.
fn find_argb_visual(screen: &x11rb::protocol::xproto::Screen) -> Option<(u8, u32)> {
    for depth in &screen.allowed_depths {
        if depth.depth == 32 {
            if let Some(v) = depth.visuals.first() {
                return Some((32, v.visual_id));
            }
        }
    }
    None
}

/// Locate the gamescope nested-X display advertising the overlay atom (same
/// probe the `doctor` uses).
fn discover_gamescope_display() -> Option<String> {
    let mut nums: Vec<u32> = std::fs::read_dir("/tmp/.X11-unix")
        .ok()?
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().to_str().and_then(|s| s.strip_prefix('X').map(str::to_string)))
        .filter_map(|s| s.parse().ok())
        .collect();
    nums.sort_unstable();
    for n in nums {
        let dpy = format!(":{n}");
        if let Ok((conn, _)) = x11rb::connect(Some(&dpy)) {
            if let Ok(cookie) = conn.intern_atom(true, b"GAMESCOPE_EXTERNAL_OVERLAY") {
                if cookie.reply().map(|r| r.atom != 0).unwrap_or(false) {
                    return Some(dpy);
                }
            }
        }
    }
    None
}
