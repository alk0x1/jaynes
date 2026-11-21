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

/// How wide the edge the person drags is, inside its bar.
pub const EDGE: f32 = 6.;

/// What the right side shows after the person presses `pressed`, with `now` there: one panel at a time. The panel
/// button (Bar::Steps, ⌘E) is the right side's own: whatever shows, it closes it, and with nothing there it opens the
/// steps. Changes' switch opens Changes, and closes them to nothing when they show. (Before, the button switched the
/// steps hidden under Changes, so the side never closed.)
pub fn right_after(now: Option<Bar>, pressed: Bar) -> Option<Bar> {
    match (now, pressed) {
        (Some(_), Bar::Steps) => None,
        (None, Bar::Steps) => Some(Bar::Steps),
        (Some(Bar::Changes), Bar::Changes) => None,
        (_, Bar::Changes) => Some(Bar::Changes),
        (now, Bar::Column) => now,
    }
}

/// Whether a drag of a panel at the right that asks `wanted` closes it: dragged under half its lowest width, as the
/// column dragged narrow becomes the rail. The column never closes so.
pub fn drag_closes(bar: Bar, wanted: f32) -> bool {
    bar.from_right() && wanted < bar.bounds().1 / 2.
}

/// What shows beside the middle: the column or not, and the bar at the right (Changes over the steps), if any.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shown {
    pub column: bool,
    pub right: Option<Bar>,
}

/// The bars' widths as the window stands.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bars {
    pub column: f32,
    pub steps: f32,
    pub changes: f32,
}

impl Default for Bars {
    fn default() -> Self {
        Bars { column: Bar::Column.default_width(), steps: Bar::Steps.default_width(), changes: Bar::Changes.default_width() }
    }
}

impl Bars {
    pub fn of(&self, bar: Bar) -> f32 {
        match bar {
            Bar::Column => self.column,
            Bar::Steps => self.steps,
            Bar::Changes => self.changes,
        }
    }

    fn at(&mut self, bar: Bar) -> &mut f32 {
        match bar {
            Bar::Column => &mut self.column,
            Bar::Steps => &mut self.steps,
            Bar::Changes => &mut self.changes,
        }
    }

    /// The widths kept, each within its bounds, and why one opens at its own.
    pub fn from_kept(k: crate::app_state::Widths) -> (Bars, Vec<String>) {
        let mut why = Vec::new();
        let mut take = |bar, w| {
            let (w, said) = kept(bar, w);
            why.extend(said);
            w
        };
        let bars = Bars { column: take(Bar::Column, k.column), steps: take(Bar::Steps, k.steps), changes: take(Bar::Changes, k.changes) };
        (bars, why)
    }

    /// As kept: none for a bar at its own width.
    pub fn to_kept(&self) -> crate::app_state::Widths {
        let keep = |bar: Bar| Some(self.of(bar)).filter(|w| *w != bar.default_width());
        crate::app_state::Widths { column: keep(Bar::Column), steps: keep(Bar::Steps), changes: keep(Bar::Changes) }
    }

    /// The width of what shares the window with the bar across the middle: for the column, the bar at the right;
    /// for a bar at the right, the column.
    fn across(&self, bar: Bar, shown: Shown) -> f32 {
        match bar {
            Bar::Column => shown.right.map_or(0., |b| self.of(b)),
            _ if shown.column => self.column,
            _ => 0.,
        }
    }

    /// The bar's edge dragged with the pointer at `x` in a window `window` wide; whether its width changed.
    pub fn drag(&mut self, bar: Bar, x: f32, window: f32, shown: Shown) -> bool {
        let w = dragged(bar, wanted_at(bar, x, window), window, self.across(bar, shown));
        std::mem::replace(self.at(bar), w) != w
    }

    /// A double click on the bar's edge: its own width back.
    pub fn reset(&mut self, bar: Bar) {
        *self.at(bar) = bar.default_width();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn both() -> Shown {
        Shown { column: true, right: Some(Bar::Steps) }
    }

    #[test]
    fn a_drag_of_an_edge_changes_that_bar_alone_and_a_double_click_gives_its_own_back() {
        let mut b = Bars::default();
        // the column's edge to 400, the steps' edge to 1000 in a 1600 window: 600 wide
        assert!(b.drag(Bar::Column, 400., 1600., both()));
        assert!(b.drag(Bar::Steps, 1000., 1600., both()));
        assert_eq!((b.column, b.steps, b.changes), (400., 560., 360.));
        // the same place again changes nothing
        assert!(!b.drag(Bar::Column, 400., 1600., both()));
        b.reset(Bar::Steps);
        assert_eq!(b.steps, 300.);
        assert_eq!(b.column, 400.);
    }

    #[test]
    fn the_middle_keeps_its_width_beside_what_shows_and_not_beside_what_is_hidden() {
        let mut b = Bars { column: 480., ..Bars::default() };
        // beside a 480 column a 1200 window leaves the steps 1200 - 480 - 320 = 400
        b.drag(Bar::Steps, 100., 1200., both());
        assert_eq!(b.steps, 400.);
        // the column hidden: the steps take up to their highest
        b.drag(Bar::Steps, 100., 1200., Shown { column: false, right: Some(Bar::Steps) });
        assert_eq!(b.steps, 560.);
        // Changes at the right is what the column shares the window with, not the steps hidden under it
        let mut b = Bars { changes: 640., steps: 160., ..Bars::default() };
        b.drag(Bar::Column, 700., 1200., Shown { column: true, right: Some(Bar::Changes) });
        assert_eq!(b.column, 240.);
        b.drag(Bar::Column, 700., 1200., Shown { column: true, right: None });
        assert_eq!(b.column, 480.);
    }

    #[test]
    fn the_widths_are_kept_only_where_the_person_changed_them_and_come_back_so() {
        let (b, why) = Bars::from_kept(crate::app_state::Widths::default());
        assert_eq!((b, why.len()), (Bars::default(), 0));
        assert_eq!(Bars::default().to_kept(), crate::app_state::Widths::default());
        let dragged = Bars { column: RAIL, steps: 410., ..Bars::default() };
        let k = dragged.to_kept();
        assert_eq!(k, crate::app_state::Widths { column: Some(RAIL), steps: Some(410.), changes: None });
        assert_eq!(Bars::from_kept(k), (dragged, vec![]));
        // one out of its bounds opens at its own, the others as kept, and the log says which
        let (b, why) = Bars::from_kept(crate::app_state::Widths { column: Some(9000.), steps: Some(410.), changes: None });
        assert_eq!(b, Bars { steps: 410., ..Bars::default() });
        assert_eq!(why.len(), 1);
        assert!(why[0].contains("Column"), "{why:?}");
    }

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

    #[test]
    fn the_panel_button_closes_the_right_side_whatever_shows_there() {
        // Changes over the steps was what the button could not close: it switched the steps hidden under Changes
        assert_eq!(right_after(Some(Bar::Changes), Bar::Steps), None);
        assert_eq!(right_after(Some(Bar::Steps), Bar::Steps), None);
        // with nothing at the right, it opens the steps
        assert_eq!(right_after(None, Bar::Steps), Some(Bar::Steps));
    }

    #[test]
    fn changes_switch_opens_changes_and_closes_them_to_nothing() {
        assert_eq!(right_after(None, Bar::Changes), Some(Bar::Changes));
        assert_eq!(right_after(Some(Bar::Steps), Bar::Changes), Some(Bar::Changes));
        // closed, Changes leave the right side empty, never the steps that were under them
        assert_eq!(right_after(Some(Bar::Changes), Bar::Changes), None);
    }

    #[test]
    fn a_panel_at_the_right_dragged_under_half_its_lowest_closes_and_the_column_never_does() {
        // the steps' lowest is 160, Changes' 180
        assert!(!drag_closes(Bar::Steps, 80.) && drag_closes(Bar::Steps, 79.));
        assert!(!drag_closes(Bar::Changes, 90.) && drag_closes(Bar::Changes, 89.));
        assert!(drag_closes(Bar::Changes, -40.));
        // the column dragged narrow is the rail, as before
        assert!(!drag_closes(Bar::Column, 0.));
    }
}
