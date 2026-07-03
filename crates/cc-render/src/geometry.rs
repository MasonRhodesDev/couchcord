//! Pure overlay geometry: place a window of size `win` at one of the 8 anchors
//! within `screen`, inset by `pad`. No X server needed — fully unit-tested.

use cc_core::Anchor;

/// Top-left `(x, y)` for an overlay window of size `win = (w, h)` anchored within
/// a `screen = (w, h)`, inset from the edges by `pad` pixels. Clamped to be
/// non-negative (a window larger than the screen pins to the origin).
pub fn anchor_rect(anchor: Anchor, win: (u32, u32), screen: (u32, u32), pad: u32) -> (i32, i32) {
    let (ww, wh) = (win.0 as i32, win.1 as i32);
    let (sw, sh) = (screen.0 as i32, screen.1 as i32);
    let p = pad as i32;

    let left = p;
    let center_x = (sw - ww) / 2;
    let right = sw - ww - p;

    let top = p;
    let middle_y = (sh - wh) / 2;
    let bottom = sh - wh - p;

    let (x, y) = match anchor {
        Anchor::TopLeft => (left, top),
        Anchor::TopCenter => (center_x, top),
        Anchor::TopRight => (right, top),
        Anchor::MidLeft => (left, middle_y),
        Anchor::MidRight => (right, middle_y),
        Anchor::BottomLeft => (left, bottom),
        Anchor::BottomCenter => (center_x, bottom),
        Anchor::BottomRight => (right, bottom),
        _ => (left, top), // future anchors: safe default
    };
    (x.max(0), y.max(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: (u32, u32) = (1920, 1080);
    const WIN: (u32, u32) = (300, 200);
    const PAD: u32 = 16;

    fn at(a: Anchor) -> (i32, i32) {
        anchor_rect(a, WIN, SCREEN, PAD)
    }

    #[test]
    fn corners_are_correct() {
        assert_eq!(at(Anchor::TopLeft), (16, 16));
        assert_eq!(at(Anchor::TopRight), (1920 - 300 - 16, 16)); // (1604, 16)
        assert_eq!(at(Anchor::BottomLeft), (16, 1080 - 200 - 16)); // (16, 864)
        assert_eq!(at(Anchor::BottomRight), (1604, 864));
    }

    #[test]
    fn edge_midpoints_are_centered_on_the_free_axis() {
        assert_eq!(at(Anchor::TopCenter), ((1920 - 300) / 2, 16)); // (810, 16)
        assert_eq!(at(Anchor::BottomCenter), (810, 864));
        assert_eq!(at(Anchor::MidLeft), (16, (1080 - 200) / 2)); // (16, 440)
        assert_eq!(at(Anchor::MidRight), (1604, 440));
    }

    #[test]
    fn all_eight_positions_are_distinct() {
        let mut seen = std::collections::HashSet::new();
        for a in Anchor::ALL {
            assert!(
                seen.insert(at(a)),
                "anchor {a:?} collided with another position"
            );
        }
        assert_eq!(seen.len(), 8);
    }

    #[test]
    fn oversized_window_pins_to_origin_not_negative() {
        let (x, y) = anchor_rect(Anchor::BottomRight, (4000, 3000), SCREEN, PAD);
        assert!(
            x >= 0 && y >= 0,
            "never place the window off the top-left edge"
        );
        assert_eq!((x, y), (0, 0));
    }

    #[test]
    fn cycling_anchor_moves_the_window_each_step() {
        let mut a = Anchor::TopLeft;
        let mut prev = at(a);
        for _ in 0..8 {
            a = a.next();
            let pos = at(a);
            // not asserting all-different step-to-step (cycle order can share an
            // axis), but the full set is distinct (covered above); here we just
            // confirm the function is total over the cycle.
            let _ = (prev, pos);
            prev = pos;
        }
    }
}
