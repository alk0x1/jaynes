//! A direction's picture at full size, over the compare mode, with zoom as Preview and Quick Look have it: it opens
//! whole (Fit), zooms round the pointer from Fit to four times, moves when it is larger than its box and never leaves
//! an empty edge on one side, and a click goes between Fit and actual size at the point clicked. The arithmetic and the
//! keys live here; compare.rs draws it.

/// The most it zooms: four times the picture's own size.
pub const MOST: f32 = 4.;
/// The room left round the picture at Fit, on each side.
pub const PAD: f32 = 12.;
/// One press of + or −.
pub const STEP: f32 = 1.25;

/// The picture's own size and the box it is shown in, in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sizes {
    pub pic: (f32, f32),
    pub body: (f32, f32),
}

/// How the picture is shown: its scale, and where its top left corner is in the box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub z: f32,
    pub x: f32,
    pub y: f32,
}

/// What a key does in the viewer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Does {
    In,
    Out,
    Fit,
    Actual,
    Close,
    /// The direction's next or previous screen.
    Screen(isize),
    /// The next or previous direction, at the same zoom and place.
    Direction(isize),
}

impl Sizes {
    /// The scale that shows the whole picture in the box.
    pub fn fit(&self) -> f32 {
        let (w, h) = (self.pic.0.max(1.), self.pic.1.max(1.));
        ((self.body.0 - 2. * PAD) / w).min((self.body.1 - 2. * PAD) / h).max(0.01)
    }

    /// The scales a view may take: from Fit to four times, or Fit alone for a picture so small that Fit is larger.
    fn range(&self) -> (f32, f32) {
        let fit = self.fit();
        (fit.min(MOST), fit.max(MOST))
    }

    pub fn center(&self) -> (f32, f32) {
        (self.body.0 / 2., self.body.1 / 2.)
    }
}

impl View {
    /// The picture whole, in the middle of the box.
    pub fn fitted(s: Sizes) -> View {
        View { z: s.fit(), x: 0., y: 0. }.clamped(s)
    }

    /// Kept in the box: in the middle along a side where it is smaller than the box, and where it is larger, with no
    /// empty edge on one side.
    pub fn clamped(self, s: Sizes) -> View {
        let along = |at: f32, size: f32, room: f32| if size <= room { (room - size) / 2. } else { at.clamp(room - size, 0.) };
        View { z: self.z, x: along(self.x, s.pic.0 * self.z, s.body.0), y: along(self.y, s.pic.1 * self.z, s.body.1) }
    }

    /// Zoomed to `z`, within its range, with the point `at` of the box over the same spot of the picture.
    pub fn zoomed(self, z: f32, at: (f32, f32), s: Sizes) -> View {
        let (least, most) = s.range();
        let z = z.clamp(least, most);
        let k = z / self.z;
        View { z, x: at.0 - (at.0 - self.x) * k, y: at.1 - (at.1 - self.y) * k }.clamped(s)
    }

    /// Moved by the pointer or two fingers, within the box.
    pub fn moved(self, by: (f32, f32), s: Sizes) -> View {
        View { x: self.x + by.0, y: self.y + by.1, ..self }.clamped(s)
    }

    /// Whether it shows more than Fit.
    pub fn zoomed_in(&self, s: Sizes) -> bool {
        self.z > s.fit() + 1e-3
    }

    /// A click without a drag: from Fit to actual size at the point clicked, and from any zoom back to Fit.
    pub fn clicked(self, at: (f32, f32), s: Sizes) -> View {
        if self.zoomed_in(s) { View::fitted(s) } else { self.zoomed(1., at, s) }
    }

    /// The same zoom and place on another picture (the next screen, or another direction), within its range.
    pub fn kept(self, s: Sizes) -> View {
        let (least, most) = s.range();
        View { z: self.z.clamp(least, most), ..self }.clamped(s)
    }

    /// The scale as the viewer says it.
    pub fn percent(&self) -> u32 {
        (self.z * 100.).round() as u32
    }

    /// Between `a` and `b` at `t` (0..1), for a zoom a button or a click makes.
    pub fn between(a: View, b: View, t: f32) -> View {
        let at = |p: f32, q: f32| p + (q - p) * t;
        View { z: at(a.z, b.z), x: at(a.x, b.x), y: at(a.y, b.y) }
    }
}

/// What a key does in the viewer, with Command held or not: + and − zoom, with Command too; Command-0 is actual size and
/// Command-9 Fit, as Preview has them (they are the runs column's and the ninth tab's outside the viewer); ↑ ↓ go
/// round the direction's screens and ← → round the directions; Esc closes.
pub fn key(key: &str, cmd: bool) -> Option<Does> {
    Some(match (key, cmd) {
        ("escape", _) => Does::Close,
        ("+" | "=", _) => Does::In,
        ("-", _) => Does::Out,
        ("0", true) => Does::Actual,
        ("9", true) => Does::Fit,
        ("down", false) => Does::Screen(1),
        ("up", false) => Does::Screen(-1),
        ("right", false) => Does::Direction(1),
        ("left", false) => Does::Direction(-1),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::{Does, MOST, Sizes, View, key};

    /// A picture twice as wide as high, in a box 1000 by 600.
    const S: Sizes = Sizes { pic: (2000., 1000.), body: (1000., 600.) };

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.01
    }

    /// The spot of the picture under a point of the box.
    fn under(v: View, at: (f32, f32)) -> (f32, f32) {
        ((at.0 - v.x) / v.z, (at.1 - v.y) / v.z)
    }

    #[test]
    fn it_opens_whole_in_the_middle_of_its_box() {
        let v = View::fitted(S);
        // the width decides: 976 of 2000, the picture 976 by 488 with 12 on each side and the height centred
        assert!(near(v.z, 0.488), "{v:?}");
        assert!(near(v.x, 12.) && near(v.y, 56.), "{v:?}");
        assert_eq!(v.percent(), 49);
    }

    #[test]
    fn it_zooms_round_the_pointer_from_fit_to_four_times() {
        let fit = View::fitted(S);
        let at = (500., 300.);
        let v = fit.zoomed(1., at, S);
        assert!(near(v.z, 1.), "{v:?}");
        let (a, b) = (under(fit, at), under(v, at));
        assert!(near(a.0, b.0) && near(a.1, b.1), "the spot under the pointer moved: {a:?} then {b:?}");
        assert!(near(fit.zoomed(100., at, S).z, MOST));
        assert!(near(v.zoomed(0.01, at, S).z, fit.z), "below Fit is Fit");
    }

    #[test]
    fn a_larger_picture_moves_and_never_leaves_an_empty_edge_on_one_side() {
        let v = View::fitted(S).zoomed(1., (500., 300.), S);
        // dragged far to the right, its left edge stops at the box's left edge
        let right = v.moved((5000., 0.), S);
        assert!(near(right.x, 0.), "{right:?}");
        // and far to the left, its right edge at the box's right edge
        let left = v.moved((-5000., -5000.), S);
        assert!(near(left.x, 1000. - 2000.) && near(left.y, 600. - 1000.), "{left:?}");
        // whole in the box, it stays in the middle whatever the drag
        let fit = View::fitted(S);
        assert_eq!(fit.moved((300., 200.), S), fit);
    }

    #[test]
    fn a_click_goes_from_fit_to_actual_size_at_the_point_and_back() {
        let fit = View::fitted(S);
        let at = (250., 150.);
        let actual = fit.clicked(at, S);
        assert!(near(actual.z, 1.), "{actual:?}");
        let (a, b) = (under(fit, at), under(actual, at));
        assert!(near(a.0, b.0) && near(a.1, b.1), "{a:?} then {b:?}");
        assert!(actual.zoomed_in(S));
        assert_eq!(actual.clicked(at, S), fit);
    }

    #[test]
    fn another_screen_or_direction_keeps_the_zoom_and_the_place() {
        let v = View { z: 2., x: -700., y: -300. };
        let other = Sizes { pic: (1600., 1200.), body: S.body };
        assert_eq!(v.kept(other), v);
        // a place the other picture does not reach comes back to its edge
        let far = View { z: 1., x: -1500., y: 0. };
        assert!(near(far.kept(other).x, 1000. - 1600.));
    }

    #[test]
    fn the_viewer_keys_zoom_with_command_too_and_take_command_0_and_9() {
        for (k, cmd, does) in [
            ("escape", false, Does::Close),
            ("=", true, Does::In),
            ("+", true, Does::In),
            ("=", false, Does::In),
            ("-", true, Does::Out),
            ("-", false, Does::Out),
            ("0", true, Does::Actual),
            ("9", true, Does::Fit),
            ("down", false, Does::Screen(1)),
            ("up", false, Does::Screen(-1)),
            ("right", false, Does::Direction(1)),
            ("left", false, Does::Direction(-1)),
        ] {
            assert_eq!(key(k, cmd), Some(does), "{k} with Command {cmd}");
        }
        // a 0 or a 9 alone is no zoom, and Command-left is the window's, not a flip
        assert_eq!(key("0", false), None);
        assert_eq!(key("9", false), None);
        assert_eq!(key("left", true), None);
        assert_eq!(key("a", false), None);
    }
}
