//! Software rasterization of a `Scene` into an ARGB pixel buffer with tiny-skia
//! + fontdue. Pure (no X), so the layout math is testable; the produced buffer
//! is handed to the X11 window by `window.rs`.

use cc_core::{Config, RowState, Scene};
use tiny_skia::{Color, Paint, PathBuilder, Pixmap, Rect, Transform};

/// A rendered frame: ARGB premultiplied pixels (tiny-skia native) + its size.
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>, // RGBA premultiplied, w*h*4
}

const ROW_H: f32 = 40.0;
const PAD: f32 = 12.0;
const WIDTH: f32 = 440.0;
const TITLE_H: f32 = 34.0;

/// Common system font locations, tried in order. None → text is skipped (the
/// pipeline still renders panels/highlights), logged once by the caller.
pub const FONT_CANDIDATES: &[&str] = &[
    "/usr/share/fonts/gsfonts/NimbusSans-Regular.otf",
    "/usr/share/fonts/TTF/DejaVuSans.ttf",
    "/usr/share/fonts/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/liberation/LiberationSans-Regular.ttf",
    "/usr/share/fonts/TTF/LiberationSans-Regular.ttf",
];

/// Load the first available system font, if any.
pub fn load_font() -> Option<fontdue::Font> {
    for path in FONT_CANDIDATES {
        if let Ok(bytes) = std::fs::read(path) {
            if let Ok(f) = fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default()) {
                return Some(f);
            }
        }
    }
    None
}

fn hex(c: &str) -> Color {
    let s = c.trim_start_matches('#');
    let n = u32::from_str_radix(s, 16).unwrap_or(0);
    if s.len() == 6 {
        Color::from_rgba8(((n >> 16) & 0xff) as u8, ((n >> 8) & 0xff) as u8, (n & 0xff) as u8, 255)
    } else {
        Color::from_rgba8(0, 0, 0, 255)
    }
}

/// Compute the frame size the scene needs (menu dominates when open; else the
/// overlay's compact roster).
fn frame_size(scene: &Scene) -> (u32, u32) {
    let mut h = 0.0f32;
    if let Some(m) = &scene.menu {
        h += TITLE_H + m.rows.len() as f32 * ROW_H + PAD * 2.0;
    }
    if let Some(o) = &scene.overlay {
        if scene.menu.is_none() {
            h += TITLE_H + o.roster.members.len().max(1) as f32 * ROW_H + PAD * 2.0;
        }
    }
    ((WIDTH) as u32, h.max(ROW_H + PAD * 2.0) as u32)
}

/// Rasterize the scene with the given theme. Returns `None` if blank.
pub fn render(scene: &Scene, cfg: &Config, font: Option<&fontdue::Font>) -> Option<Frame> {
    if scene.is_blank() {
        return None;
    }
    let (w, h) = frame_size(scene);
    let mut pm = Pixmap::new(w, h)?;

    let bg = hex(&cfg.theme.bg);
    let accent = hex(&cfg.theme.accent);
    let fg = hex(&cfg.theme.fg);
    let muted = hex(&cfg.theme.muted);

    // panel background (slightly translucent so games show through faintly)
    fill_rect(&mut pm, 0.0, 0.0, w as f32, h as f32, with_alpha(bg, 235));

    let mut y = PAD;
    if let Some(m) = &scene.menu {
        draw_text(&mut pm, font, &m.title, PAD, y, 22.0, muted);
        y += TITLE_H;
        for (i, row) in m.rows.iter().enumerate() {
            if i == m.selected {
                fill_rect(&mut pm, 4.0, y, w as f32 - 8.0, ROW_H - 4.0, with_alpha(accent, 60));
                // accent left bar
                fill_rect(&mut pm, 4.0, y, 4.0, ROW_H - 4.0, accent);
            }
            let color = match row.state {
                RowState::Loading => muted,
                RowState::Active => accent,
                _ => fg,
            };
            draw_text(&mut pm, font, &row.label, PAD + 6.0, y + 8.0, 20.0, color);
            y += ROW_H;
        }
    } else if let Some(o) = &scene.overlay {
        draw_text(&mut pm, font, &o.roster.channel_name, PAD, y, 20.0, muted);
        y += TITLE_H;
        for mem in &o.roster.members {
            let dot = if mem.speaking { accent } else { muted };
            fill_circle(&mut pm, PAD + 8.0, y + ROW_H / 2.0 - 4.0, 6.0, dot);
            let color = if mem.speaking { fg } else { muted };
            draw_text(&mut pm, font, &mem.name, PAD + 26.0, y + 8.0, 18.0, color);
            y += ROW_H;
        }
    }

    Some(Frame { width: w, height: h, pixels: pm.data().to_vec() })
}

fn with_alpha(c: Color, a: u8) -> Color {
    Color::from_rgba8(
        (c.red() * 255.0) as u8,
        (c.green() * 255.0) as u8,
        (c.blue() * 255.0) as u8,
        a,
    )
}

fn fill_rect(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, color: Color) {
    let mut paint = Paint::default();
    paint.set_color(color);
    paint.anti_alias = true;
    if let Some(rect) = Rect::from_xywh(x, y, w, h) {
        pm.fill_rect(rect, &paint, Transform::identity(), None);
    }
}

fn fill_circle(pm: &mut Pixmap, cx: f32, cy: f32, r: f32, color: Color) {
    let mut paint = Paint::default();
    paint.set_color(color);
    paint.anti_alias = true;
    let mut pb = PathBuilder::new();
    pb.push_circle(cx, cy, r);
    if let Some(path) = pb.finish() {
        pm.fill_path(&path, &paint, tiny_skia::FillRule::Winding, Transform::identity(), None);
    }
}

/// Minimal left-to-right text blitter using fontdue coverage bitmaps.
fn draw_text(pm: &mut Pixmap, font: Option<&fontdue::Font>, text: &str, x: f32, y: f32, px: f32, color: Color) {
    let Some(font) = font else { return };
    let mut pen_x = x;
    let baseline = y + px; // crude baseline placement
    for ch in text.chars() {
        let (metrics, bitmap) = font.rasterize(ch, px);
        let gx = pen_x as i32 + metrics.xmin;
        let gy = baseline as i32 - metrics.height as i32 - metrics.ymin;
        blit_coverage(pm, &bitmap, metrics.width, metrics.height, gx, gy, color);
        pen_x += metrics.advance_width;
    }
}

fn blit_coverage(pm: &mut Pixmap, cov: &[u8], cw: usize, ch: usize, ox: i32, oy: i32, color: Color) {
    let pw = pm.width() as i32;
    let ph = pm.height() as i32;
    let data = pm.data_mut();
    let (cr, cg, cb) = ((color.red() * 255.0) as u32, (color.green() * 255.0) as u32, (color.blue() * 255.0) as u32);
    for j in 0..ch {
        for i in 0..cw {
            let a = cov[j * cw + i] as u32;
            if a == 0 {
                continue;
            }
            let px = ox + i as i32;
            let py = oy + j as i32;
            if px < 0 || py < 0 || px >= pw || py >= ph {
                continue;
            }
            let idx = ((py * pw + px) * 4) as usize;
            // src-over onto premultiplied RGBA
            for (k, sc) in [cr, cg, cb].into_iter().enumerate() {
                let dst = data[idx + k] as u32;
                data[idx + k] = ((sc * a + dst * (255 - a)) / 255) as u8;
            }
            let da = data[idx + 3] as u32;
            data[idx + 3] = (a + da * (255 - a) / 255).min(255) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cc_core::{Anchor, ClientId, MenuView, Overlay, Roster, Row, VoiceKind};

    fn cfg() -> Config {
        Config {
            client_id: ClientId(1),
            anchor: Anchor::TopRight,
            voice_kinds: vec![VoiceKind::Guild],
            theme: Default::default(),
        }
    }

    #[test]
    fn blank_scene_renders_nothing() {
        assert!(render(&Scene::empty(), &cfg(), None).is_none());
    }

    #[test]
    fn menu_scene_produces_a_sized_argb_buffer() {
        let scene = Scene {
            menu: Some(MenuView {
                title: "Servers".into(),
                rows: vec![
                    Row { label: "Friends".into(), icon: None, state: RowState::Normal },
                    Row { label: "Work".into(), icon: None, state: RowState::Normal },
                ],
                selected: 1,
            }),
            overlay: None,
        };
        let f = render(&scene, &cfg(), None).expect("non-blank renders");
        assert_eq!(f.width, 440);
        assert!(f.height > 0);
        assert_eq!(f.pixels.len(), (f.width * f.height * 4) as usize);
    }

    #[test]
    fn overlay_only_scene_renders_compact() {
        let scene = Scene {
            menu: None,
            overlay: Some(Overlay {
                anchor: Anchor::TopRight,
                roster: Roster { channel_name: "General".into(), members: vec![] },
            }),
        };
        let f = render(&scene, &cfg(), None).expect("overlay renders");
        assert_eq!(f.pixels.len(), (f.width * f.height * 4) as usize);
    }
}
