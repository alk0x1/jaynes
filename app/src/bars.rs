//! The window's three bars that the person widens or narrows by dragging their edge: the column at the left, and at
//! the right the steps panel or Changes. Each has a lowest and a highest width, the middle never gets narrower than
//! MIDDLE, and a double click on an edge gives the bar its own width back. The column, dragged below the width its
//! names need, becomes a rail as narrow as Herdr's: each thing its icon and its state. The widths are kept with what
//! the window shows (app_state.rs); a kept width out of its bounds opens at the bar's own.

/// The narrowest the middle gets, whatever the bars ask.
pub const MIDDLE: f32 = 320.;
/// The rail's width: an icon and its state dot.
pub const RAIL: f32 = 52.;
/// Below this, the column's names no longer fit: it becomes the rail.
pub const NAMES: f32 = 170.;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Bar {
    Column,
    Steps,
    Changes,
}

impl Bar {
    /// Its own width, its lowest and its highest.
    pub fn bounds(self) -> (f32, f32, f32) {
        match self {
            Bar::Column => (292., RAIL, 480.),
            Bar::Steps => (300., 160., 560.),
            Bar::Changes => (360., 180., 640.),
        }
    }

    pub fn default_width(self) -> f32 {
        self.bounds().0
    }

    /// Whether the bar's edge is its left one (the panels at the right are dragged by their left edge).
    pub fn from_right(self) -> bool {
        !matches!(self, Bar::Column)
    }
}

/// The width a drag asks, `wanted`, as the bar takes it: within its bounds, leaving the middle at least MIDDLE in a
/// window `window` wide where the other bar is `other` wide; the column, below the width of its names, is the rail.
pub fn dragged(bar: Bar, wanted: f32, window: f32, other: f32) -> f32 {
    let (_, low, high) = bar.bounds();
    let room = (window - other - MIDDLE).max(low);
    let w = wanted.clamp(low, high.min(room));
    if bar == Bar::Column && w < NAMES { RAIL } else { w }
}

/// The width the bar opens at, from what was kept: the kept one if within its bounds, else its own (and why).
pub fn kept(bar: Bar, width: Option<f32>) -> (f32, Option<String>) {
    let (own, low, high) = bar.bounds();
    match width {
        None => (own, None),
        Some(w) if w.is_finite() && (low..=high).contains(&w) && !(bar == Bar::Column && w < NAMES && w != RAIL) => (w, None),
        Some(w) => (own, Some(format!("the {bar:?} kept at {w} is out of {low}..{high}: it opens at {own}"))),
    }
}

/// Whether the column at this width is the rail.
pub fn is_rail(width: f32) -> bool {
    width < NAMES
}

/// Where a drag of the bar's edge, with the pointer at `x` in a window `window` wide, asks the bar's width to be.
pub fn wanted_at(bar: Bar, x: f32, window: f32) -> f32 {
    if bar.from_right() { window - x } else { x }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drag_keeps_each_bar_within_its_bounds_and_the_middle_wide_enough() {
        // in a wide window the bar takes what is asked, up to its highest
        assert_eq!(dragged(Bar::Steps, 400., 1600., 292.), 400.);
        assert_eq!(dragged(Bar::Steps, 900., 1600., 292.), 560.);
        assert_eq!(dragged(Bar::Steps, 20., 1600., 292.), 160.);
        // in a narrow one the middle keeps its MIDDLE: 1000 - 292 - 320 = 388
        assert_eq!(dragged(Bar::Steps, 500., 1000., 292.), 388.);
        assert_eq!(dragged(Bar::Changes, 900., 1000., 292.), 388.);
        // the column at the left the same way
        assert_eq!(dragged(Bar::Column, 600., 1600., 300.), 480.);
        assert_eq!(dragged(Bar::Column, 400., 900., 300.), 280.);
    }

    #[test]
    fn the_column_narrower_than_its_names_is_the_rail() {
        assert_eq!(dragged(Bar::Column, 200., 1600., 300.), 200.);
        assert_eq!(dragged(Bar::Column, 150., 1600., 300.), RAIL);
        assert_eq!(dragged(Bar::Column, 10., 1600., 300.), RAIL);
        assert!(is_rail(RAIL) && !is_rail(200.));
        // dragged out again, the names come back
        assert_eq!(dragged(Bar::Column, 240., 1600., 300.), 240.);
    }

    #[test]
    fn a_kept_width_out_of_its_bounds_opens_at_the_bars_own_and_says_why() {
        assert_eq!(kept(Bar::Steps, None), (300., None));
        assert_eq!(kept(Bar::Steps, Some(420.)), (420., None));
        assert_eq!(kept(Bar::Column, Some(RAIL)), (RAIL, None));
        for bad in [Some(5000.), Some(10.), Some(f32::NAN), Some(-3.)] {
            let (w, why) = kept(Bar::Changes, bad);
            assert_eq!(w, 360.);
            assert!(why.is_some());
        }
        // a column between the rail and its names is no width it can stand at
        assert_eq!(kept(Bar::Column, Some(100.)).0, 292.);
    }

    #[test]
    fn the_panels_at_the_right_are_dragged_by_their_left_edge() {
        assert_eq!(wanted_at(Bar::Column, 250., 1400.), 250.);
        assert_eq!(wanted_at(Bar::Steps, 1000., 1400.), 400.);
    }
}
