//! JAYNES: copied from herdr-app's src/terminal.rs (Apache-2.0) and changed: no mods (their drags, actions and links to
//! other terminals), and its tests stay in herdr-app.
//!
//! A pane showing one core terminal: bytes in from the core, frames drawn
//! with GPUI, keys and mouse encoded by libghostty-vt and sent back.

mod drag_paths;

use crate::hc::{Core, Stream};
use crate::vt::{self, CursorShape, Rgb, Select, Vt};
use ghostty_sys::*;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_base::input::{Input, InputEditorStyle, InputEvent, InputState};
use serde_json::{Value, json};
use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use std::collections::HashMap;
use std::future::Future;
use std::task::Poll;
use crate::links;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

const OUTPUT_TIME_PER_TURN: Duration = Duration::from_millis(6);

fn output_turn(
    first: Stream,
    rx: &async_channel::Receiver<Stream>,
    budget: Duration,
) -> impl Iterator<Item = Stream> + '_ {
    let deadline = Instant::now() + budget;
    let mut first = Some(first);
    let mut ended = false;
    std::iter::from_fn(move || {
        let item = if let Some(first) = first.take() {
            first
        } else {
            // Released cores send frames of at most 16 KiB; a turn can overshoot by one frame.
            if ended || Instant::now() >= deadline { return None; }
            rx.try_recv().ok()?
        };
        ended = !matches!(item, Stream::Data(_));
        Some(item)
    })
}

fn pad() -> (f32, f32) {
    let p = crate::settings::get().terminal_padding;
    let zoom = crate::settings::zoom();
    (p[0] * zoom, p[1] * zoom)
}

pub(crate) fn grid_size(bounds: Size<Pixels>, cell: Size<Pixels>) -> (u16, u16) {
    let (px_, py_) = pad();
    let cols = ((f32::from(bounds.width) - 2.0 * px_) / f32::from(cell.width)).floor().max(2.0) as u16;
    let rows = ((f32::from(bounds.height) - 2.0 * py_) / f32::from(cell.height)).floor().max(1.0) as u16;
    (cols, rows)
}

pub fn hsla(c: Rgb) -> Hsla {
    Rgba { r: c.0 as f32 / 255.0, g: c.1 as f32 / 255.0, b: c.2 as f32 / 255.0, a: 1.0 }.into()
}

#[derive(Clone, Debug, PartialEq)]
enum LinkTarget {
    External(links::Target),
    Terminal(String),
}

pub enum TerminalEvent {
    Peek(String),
    TitleChanged,
    SelectionChanged,
    /// History scroll position changed enough for mods that watch `panes().scroll`.
    ScrollChanged,
    /// The program ended.
    Ended,
    /// Right-click where the program doesn't take the mouse: the pane menu.
    ContextMenu(Point<Pixels>),
    /// Something the person asked for failed: an error toast.
    Notice { title: String, body: String },
}

/// Rows of scrollback history as Ghostty's scrollbar reports them. `from_bottom`
/// is 0 when the viewport is at (or past) the end; absent on the alternate screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScrollMetrics {
    pub total: u64,
    pub offset: u64,
    pub len: u64,
    pub from_bottom: u64,
}

impl ScrollMetrics {
    pub fn json(self) -> Value {
        json!({
            "total": self.total,
            "offset": self.offset,
            "len": self.len,
            "from_bottom": self.from_bottom,
        })
    }
}

fn scroll_state_changed(previous: Option<u64>, current: Option<u64>) -> bool {
    previous.map(|n| n > 0) != current.map(|n| n > 0)
}

/// What the canvas closures need, shared outside the entity so prepaint can resize.
struct Grid {
    vt: Vt,
    core: Arc<Core>,
    channel: Option<u32>,
    attach_ready: bool,
    fit_ready: Option<async_channel::Sender<()>>,
    cell: Size<Pixels>,
    /// The window's scale factor, from the last prepaint. Programs see cells,
    /// images and pixel mouse reports in device pixels, as in Ghostty.
    scale: f32,
    /// The cell in device pixels the local screen was sized with.
    vt_cell: (u32, u32),
    /// The colors and device-pixel cell size the core was last told.
    told_display: Option<DisplayFacts>,
    /// Top-left of the cell grid in window coordinates, from the last paint.
    origin: Point<Pixels>,
    buttons_down: Vec<MouseButton>,
    /// Kitty images uploaded for drawing, by (image, generation, source rect).
    images: HashMap<(u32, u64, [u32; 4]), Arc<RenderImage>>,
    /// Where the pointer is over the pane.
    hover: Option<Point<Pixels>>,
    /// The link under the pointer to underline: (row, first column, last column).
    link_spans: vt::Spans,
    /// Text an input method is still composing ("nihao" before "\u{4f60}\u{597d}"),
    /// drawn at the cursor; the program sees only the finished text.
    marked: Option<String>,
    /// The cursor's cell in window coordinates, from the last paint: where the
    /// input method puts its candidate window.
    cursor_at: Option<Bounds<Pixels>>,
    /// The scroll thumb's grabbable lane from the last paint, in window
    /// coordinates (wider than the few px drawn).
    thumb: Option<Bounds<Pixels>>,
    track: Option<Bounds<Pixels>>,
    /// Each row's drawing by position, kept while the row is the same (`Rc`).
    painted: Vec<(Rc<Vec<vt::Cell>>, Vec<RowOp>, Option<usize>)>,
    /// Font size, cell size, cell font, loaded fonts and ligature setting used to draw.
    painted_with: (u32, Size<Pixels>, Font, u64, bool),
}

pub struct TerminalView {
    pub address: String,
    pub title: String,
    pub ended: Option<String>,
    /// The core connection dropped; waiting to reattach.
    pub lost: bool,
    /// The core's first paint of the screen has come in since the last attach.
    screen: bool,
    /// A left-button selection drag is in progress.
    selecting: bool,
    published_selection: Option<vt::SelectionPositions>,
    /// A scroll thumb drag is: the px into the thumb it was grabbed at.
    scrollbar_grab: Option<f32>,
    selection_pointer: Point<Pixels>,
    selection_scroll: Option<Task<()>>,
    /// The terminal's folder, for links to relative paths.
    pub cwd: Option<PathBuf>,
    /// The link under the pointer while the link modifier is held.
    link: Option<LinkTarget>,
    /// Right-clicks go to the program when it takes the mouse (Shift: the menu).
    pub right_click_to_program: bool,
    /// Files on their way to this terminal's machine for over a second: what
    /// the pane says meanwhile, by send.
    sending: Vec<(u64, &'static str)>,
    grid: Rc<RefCell<Grid>>,
    focus: FocusHandle,
    was_focused: bool,
    /// The window frame this terminal was last put on screen in (`FRAME`).
    shown_in: Cell<u64>,
    /// When the person last typed here: output right after is shown at once.
    typed_at: Cell<Option<Instant>>,
    /// Scrolled this gesture, in lines, toward the next whole one.
    scrolled: Point<f32>,
    /// Search UI and query belong to this terminal view, not the window. Each
    /// terminal in a tab therefore keeps its own query and match position.
    search_open: bool,
    /// Restore the last keyboard target when returning to this pane.
    search_keys: bool,
    search_query: String,
    search_input: Option<Entity<InputState>>,
    pending_search_nav: Option<bool>,
    search_task: Option<Task<()>>,
    attach_deadline: Instant,
    /// Last observed distance; output only publishes transitions to/from the end.
    published_from_bottom: Option<u64>,
    _render_hold: Option<(Instant, Task<()>)>,
    _reader: Option<Task<()>>,
}

thread_local! {
    /// Terminals with output waiting for the next tick of the output clock.
    static OUTPUT_CLOCK: RefCell<Vec<WeakEntity<TerminalView>>> = const { RefCell::new(Vec::new()) };
}

/// The window's frame number, counted where the window renders.
static FRAME: AtomicU64 = AtomicU64::new(0);

pub fn next_frame() {
    FRAME.fetch_add(1, Ordering::Relaxed);
}

pub fn flush_output_clock(cx: &mut gpui::App) {
    for view in OUTPUT_CLOCK.with(|c| std::mem::take(&mut *c.borrow_mut())) {
        let _ = view.update(cx, |_, cx| cx.notify());
    }
}

impl EventEmitter<TerminalEvent> for TerminalView {}

#[derive(Clone, Copy, PartialEq, Eq)]
struct DisplayFacts {
    fg: Rgb,
    bg: Rgb,
    cursor: Rgb,
    /// In device pixels, what the core reports to programs (CSI 14t, 16t).
    cell: (u32, u32),
}

/// Only send changed facts; a theme switch need not change the cell size.
fn display_update(told: &mut Option<DisplayFacts>, theme: &vt::Theme, cell: (u32, u32)) -> Option<serde_json::Value> {
    let facts = DisplayFacts { fg: theme.fg, bg: theme.bg, cursor: theme.cursor, cell };
    if *told == Some(facts) {
        return None;
    }
    *told = Some(facts);
    let hex = |c: Rgb| format!("#{:02x}{:02x}{:02x}", c.0, c.1, c.2);
    Some(json!({"fg": hex(facts.fg), "bg": hex(facts.bg), "cursor": hex(facts.cursor), "cell": [cell.0, cell.1]}))
}

pub fn initial_display(window: &Window) -> serde_json::Value {
    display_update(&mut None, &vt::theme(), device_cell(cell_size(window), window.scale_factor())).unwrap()
}

impl Grid {
    fn ready_to_attach(&mut self) {
        self.attach_ready = true;
        if let Some(ready) = self.fit_ready.take() {
            let _ = ready.try_send(());
        }
    }

    fn refresh_theme(&mut self, theme: &vt::Theme, cell: (u32, u32)) {
        self.vt.set_theme(theme.clone());
        if let Some(chan) = self.channel
            && let Some(facts) = display_update(&mut self.told_display, theme, cell)
        {
            self.core.facts(chan, &facts);
        }
    }

    /// Sizes the screen to `cols` x `rows` of `cell` at `scale`; the program hears of a change.
    fn fit(&mut self, cols: u16, rows: u16, cell: Size<Pixels>, scale: f32) {
        self.cell = cell;
        self.scale = scale;
        let dev = device_cell(cell, scale);
        // Before the size, so a program woken by it asks for the new cell.
        self.refresh_theme(&vt::theme(), dev);
        self.ready_to_attach();
        if (cols, rows) == (self.vt.cols, self.vt.rows) && dev == self.vt_cell {
            return;
        }
        self.vt.resize(cols, rows, dev.0, dev.1);
        self.vt_cell = dev;
        if let Some(chan) = self.channel {
            self.core.resize(chan, cols, rows);
            // Mode 2048 programs wait for this report, not SIGWINCH.
            let replies = self.vt.take_replies();
            self.core.automatic_input(chan, &replies);
        }
    }
}

/// A cell in device pixels.
fn device_cell(cell: Size<Pixels>, scale: f32) -> (u32, u32) {
    let px = |v: Pixels| (f32::from(v) * scale).round().max(1.0) as u32;
    (px(cell.width), px(cell.height))
}

// Held-button events belong to the pane that received the press.
fn track_mouse_button(buttons: &mut Vec<MouseButton>, action: GhosttyMouseAction, button: Option<MouseButton>) -> bool {
    if let Some(button) = button {
        if action == GHOSTTY_MOUSE_ACTION_PRESS {
            if !buttons.contains(&button) {
                buttons.push(button);
            }
        } else if !buttons.contains(&button) {
            return false;
        } else if action == GHOSTTY_MOUSE_ACTION_RELEASE {
            buttons.retain(|b| *b != button);
        }
    }
    true
}

impl TerminalView {
    /// Waits for pane layout before replay, bounded so an unpainted view still attaches.
    pub fn attach(core: Arc<Core>, address: String, size_hint: (u16, u16), cx: &mut Context<Self>) -> Self {
        let attach_deadline = cx.background_executor().now() + Duration::from_millis(100);
        let mut view = TerminalView {
            address: address.clone(),
            title: address.clone(),
            ended: None,
            lost: false,
            screen: false,
            selecting: false,
            published_selection: None,
            scrollbar_grab: None,
            selection_pointer: Point::default(),
            selection_scroll: None,
            cwd: None,
            link: None,
            right_click_to_program: false,
            sending: Vec::new(),
            grid: Rc::new(RefCell::new(Grid {
                vt: Vt::new(size_hint.0.max(2), size_hint.1.max(1)),
                core: core.clone(),
                channel: None,
                attach_ready: false,
                fit_ready: None,
                cell: size(px(9.0), px(18.0)),
                scale: 1.0,
                vt_cell: (9, 18),
                told_display: None,
                origin: Point::default(),
                buttons_down: Vec::new(),
                images: HashMap::new(),
                hover: None,
                link_spans: Vec::new(),
                marked: None,
                cursor_at: None,
                thumb: None,
                track: None,
                painted: Vec::new(),
                painted_with: (0, Size::default(), font(""), 0, false),
            })),
            focus: cx.focus_handle(),
            was_focused: false,
            shown_in: Cell::new(0),
            typed_at: Cell::new(None),
            scrolled: point(0.0, 0.0),
            search_open: false,
            search_keys: false,
            search_query: String::new(),
            search_input: None,
            pending_search_nav: None,
            search_task: None,
            attach_deadline,
            published_from_bottom: None,
            _render_hold: None,
            _reader: None,
        };
        view.connect(core, cx);
        view
    }

    /// Attaches to the same terminal on a (new) core connection.
    pub fn reattach(&mut self, core: Arc<Core>, cx: &mut Context<Self>) {
        if matches!(self.ended.as_deref(), Some("exited" | "killed" | "finished")) {
            return;
        }
        self.ended = None;
        self.lost = false;
        self.connect(core, cx);
        cx.notify();
    }

    /// A kept terminal can restart without its core connection changing.
    pub fn reattach_if_needed(&mut self, core: Arc<Core>, cx: &mut Context<Self>) {
        if self.lost || self.ended.is_some() { self.reattach(core, cx); }
    }

    /// Attaches off the UI thread: a terminal on a machine still dialing can
    /// take seconds, and the window must keep drawing meanwhile.
    fn connect(&mut self, core: Arc<Core>, cx: &mut Context<Self>) {
        self._render_hold = None;
        self.screen = false;
        let fit_ready = {
            let mut g = self.grid.borrow_mut();
            let (cols, rows) = (g.vt.cols, g.vt.rows);
            // The core repaints the whole screen (and history) on attach.
            let cell = device_cell(g.cell, g.scale);
            g.vt = Vt::new(cols, rows);
            g.vt.resize(cols, rows, cell.0, cell.1);
            if self.search_open {
                g.vt.start_search(&self.search_query);
            }
            g.vt_cell = cell;
            g.told_display = None;
            g.core = core.clone();
            g.channel = None;
            if g.attach_ready {
                None
            } else {
                let (ready, wait) = async_channel::bounded(1);
                g.fit_ready = Some(ready);
                Some(wait)
            }
        };
        self.selection_changed(cx);
        let address = self.address.clone();
        let deadline = cx.background_executor().timer(self.attach_deadline.saturating_duration_since(cx.background_executor().now()));
        // Replacing the task drops an attach still in flight for an older connection.
        self._reader = Some(cx.spawn(async move |this, cx| {
            if let Some(ready) = fit_ready {
                let mut ready = std::pin::pin!(ready.recv());
                let mut deadline = std::pin::pin!(deadline);
                std::future::poll_fn(|cx| {
                    if ready.as_mut().poll(cx).is_ready() || deadline.as_mut().poll(cx).is_ready() {
                        Poll::Ready(())
                    } else {
                        Poll::Pending
                    }
                }).await;
            }
            let Ok((cols, rows, facts)) = this.update(cx, |this, _| {
                let mut g = this.grid.borrow_mut();
                g.ready_to_attach();
                let cell = device_cell(g.cell, g.scale);
                let facts = display_update(&mut g.told_display, &vt::theme(), cell).unwrap();
                (g.vt.cols, g.vt.rows, facts)
            }) else { return };
            let attached = cx.background_executor().spawn(async move { core.attach(&address, cols, rows, facts) }).await;
            let Ok(Some(rx)) = this.update(cx, |this, cx| this.attached(attached, (cols, rows), cx)) else {
                return;
            };
            // Keep the first history paint together; live output gets the shorter budget.
            let mut budget = Duration::from_millis(250);
            while let Ok(first) = rx.recv().await {
                let alive = this.update(cx, |this, cx| this.ingest(output_turn(first, &rx, budget), cx));
                budget = OUTPUT_TIME_PER_TURN;
                if !matches!(alive, Ok(true)) {
                    break;
                }
                if !rx.is_empty() {
                    // A ready receiver does not yield to input and painting.
                    let mut yielded = false;
                    std::future::poll_fn(|cx| {
                        if std::mem::replace(&mut yielded, true) {
                            Poll::Ready(())
                        } else {
                            cx.waker().wake_by_ref();
                            Poll::Pending
                        }
                    }).await;
                }
            }
        }));
    }

    fn attached(
        &mut self,
        attached: anyhow::Result<crate::hc::Attached>,
        attached_size: (u16, u16),
        cx: &mut Context<Self>,
    ) -> Option<async_channel::Receiver<Stream>> {
        cx.notify();
        match attached {
            Ok(attached) => {
                self.lost = false;
                let mut g = self.grid.borrow_mut();
                // Replay uses the size requested by attach, not a newer pane layout.
                if (g.vt.cols, g.vt.rows) != attached_size {
                    g.core.detach(attached.channel);
                    let core = g.core.clone();
                    drop(g);
                    self.connect(core, cx);
                    return None;
                }
                g.channel = Some(attached.channel);
                // The theme or cell may have changed while the attach was in flight.
                let cell = device_cell(g.cell, g.scale);
                g.refresh_theme(&vt::theme(), cell);
                drop(g);
                if let Some(t) = attached.terminal.get("title").and_then(|t| t.as_str()).filter(|t| !t.is_empty()) {
                    self.set_title(t, cx);
                }
                cx.emit(TerminalEvent::SelectionChanged);
                Some(attached.stream)
            }
            // Gone for good (removed, finished, unknown) versus a connection problem.
            Err(e) if crate::hc::error_kind(&e) == Some("unavailable") || format!("{e:#}").contains("lost the core") || format!("{e:#}").contains("no reply") => {
                self.lost = true;
                None
            }
            // It ended between being listed and being attached: its pane goes,
            // like one whose program exits while shown.
            Err(e) if crate::hc::error_kind(&e) == Some("finished") => {
                self.ended = Some("finished".into());
                cx.emit(TerminalEvent::Ended);
                None
            }
            Err(e) => {
                // A failed lookup or reconnect is not evidence the program exited.
                self.ended = Some(format!("{e:#}"));
                None
            }
        }
    }

    /// Attached tabs follow the window's theme even when they are not drawn.
    pub fn refresh_theme(&self) {
        let mut g = self.grid.borrow_mut();
        let cell = device_cell(g.cell, g.scale);
        g.refresh_theme(&vt::theme(), cell);
    }

    /// Its title as shown (see `shown_title`); news only when that changed.
    fn set_title(&mut self, title: &str, cx: &mut Context<Self>) {
        let shown = shown_title(title);
        if shown != self.title {
            self.title = shown.into_owned();
            cx.emit(TerminalEvent::TitleChanged);
        }
    }

    /// Something to show: the screen the core painted on attach, or why there is none (it ended, the line dropped).
    pub fn has_screen(&self) -> bool {
        self.screen || self.ended.is_some() || self.lost
    }

    #[cfg(test)]
    pub(crate) fn dimensions(&self) -> (u16, u16) {
        let grid = self.grid.borrow();
        (grid.vt.cols, grid.vt.rows)
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    /// The focused terminal remains the action target while its local find
    /// input owns keyboard focus (notably for search, copy and paste commands).
    pub fn owns_keyboard_focus(&self, window: &Window, cx: &App) -> bool {
        self.focus.is_focused(window) || self.find_field_focused(window, cx)
    }

    pub fn focus_target(&self, cx: &App) -> FocusHandle {
        if self.search_open && self.search_keys
            && let Some(input) = &self.search_input
        {
            return input.read(cx).focus_handle(cx);
        }
        self.focus.clone()
    }

    fn find_field_focused(&self, window: &Window, cx: &App) -> bool {
        self.search_open
            && self.search_input.as_ref().is_some_and(|input| input.read(cx).focus_handle(cx).is_focused(window))
    }

    pub fn detach(&self) {
        let g = self.grid.borrow();
        if let Some(ch) = g.channel {
            g.core.detach(ch);
        }
    }

    /// Feeds relayed bytes in; returns false once the stream is over.
    fn ingest(&mut self, batch: impl IntoIterator<Item = Stream>, cx: &mut Context<Self>) -> bool {
        let mut open = true;
        let had_screen = self.screen;
        let title = {
            let mut g = self.grid.borrow_mut();
            for item in batch {
                match item {
                    Stream::Data(bytes) => {
                        self.screen = true;
                        g.vt.allow_clipboard_write(crate::settings::get().clipboard_write);
                        g.vt.write(&bytes);
                        let [clipboard, primary] = g.vt.take_clipboard();
                        if let Some(text) = clipboard {
                            cx.write_to_clipboard(ClipboardItem::new_string(text));
                        }
                        #[cfg(target_os = "linux")]
                        if let Some(text) = primary {
                            cx.write_to_primary(ClipboardItem::new_string(text));
                        }
                        #[cfg(not(target_os = "linux"))]
                        let _ = primary;
                        if let Some(ch) = g.channel {
                            g.core.credit(ch, bytes.len());
                            // What the core leaves to viewers (e.g. kitty graphics replies).
                            let replies = g.vt.take_replies();
                            g.core.automatic_input(ch, &replies);
                        }
                    }
                    Stream::Closed(why) => {
                        open = false;
                        g.channel = None;
                        self.ended = Some(why.get("reason").and_then(|r| r.as_str()).unwrap_or("ended").to_owned());
                    }
                    Stream::Lost => {
                        g.channel = None;
                        self.lost = true;
                        cx.notify();
                        return false;
                    }
                }
            }
            g.vt.take_title()
        };
        if let Some(title) = title {
            let title = if title.is_empty() { self.address.clone() } else { title };
            self.set_title(&title, cx);
        }
        self.selection_changed(cx);
        // whoever waits to show it (the agent shown before stays over it until then) hears at once
        if self.screen && !had_screen {
            cx.notify();
        }
        if !open {
            cx.emit(TerminalEvent::Ended);
        }
        // GPUI can retain this pane's drawing while other panes paint.
        // Wake it at the sync deadline even if the program sends no more bytes.
        let deadline = self.grid.borrow().vt.render_hold_deadline();
        if self._render_hold.as_ref().map(|(deadline, _)| *deadline) != deadline {
            self._render_hold = deadline.map(|deadline| (deadline, cx.spawn(async move |this, cx| {
                cx.background_executor().timer(deadline.saturating_duration_since(Instant::now())).await;
                let _ = this.update(cx, |this, cx| {
                    if this.shown_in.get() == FRAME.load(Ordering::Relaxed) {
                        cx.notify();
                    }
                });
            })));
        }
        self.publish_scroll_if_changed(cx);
        // Output in a terminal nobody sees doesn't redraw the window; it's
        // drawn fresh when shown again. Nor does output that changed nothing
        // on screen (an agent spinning its title).
        if self.shown_in.get() == FRAME.load(Ordering::Relaxed) && self.grid.borrow_mut().vt.shows_change() {
            self.redraw_soon(cx);
        }
        open
    }

    /// Output is drawn on a shared clock (~30 a second), so panes printing
    /// out of step share window frames; right after typing, at once.
    fn redraw_soon(&self, cx: &mut Context<Self>) {
        if self.typed_at.get().is_some_and(|t| t.elapsed() < Duration::from_millis(200)) {
            cx.notify();
            return;
        }
        let me = cx.entity().downgrade();
        let start = OUTPUT_CLOCK.with(|c| {
            let mut c = c.borrow_mut();
            if !c.iter().any(|w| w.entity_id() == me.entity_id()) {
                c.push(me);
            }
            c.len() == 1
        });
        if start {
            cx.spawn(async |_, cx| {
                cx.background_executor().timer(Duration::from_millis(33)).await;
                cx.update(flush_output_clock);
            })
            .detach();
        }
    }

    /// The window puts this terminal on screen in the frame it's building.
    pub fn mark_shown(&self) {
        self.shown_in.set(FRAME.load(Ordering::Relaxed));
    }

    /// A hidden pane cannot render its focus loss.
    pub fn blur(&mut self) {
        if self.was_focused {
            self.was_focused = false;
            let g = self.grid.borrow();
            if let Some(ch) = g.channel {
                g.core.focus(ch, false);
            }
        }
    }

    /// Sizes a terminal that isn't on screen as its pane would be (`AppWindow::fit_hidden_tabs`).
    pub fn fit(&self, cols: u16, rows: u16, cell: Size<Pixels>, scale: f32) {
        self.grid.borrow_mut().fit(cols, rows, cell, scale);
    }

    pub fn on_screen_this_frame(&self) -> bool {
        self.shown_in.get() == FRAME.load(Ordering::Relaxed)
    }

    /// JAYNES: types text into the terminal as a paste would, for the app's own buttons ("+ New run").
    pub fn type_text(&self, text: &str) {
        let bytes = self.grid.borrow().vt.paste_bytes(text);
        self.send(&bytes);
    }

    /// Presses keys as the keyboard would: the bytes go to the program as they are, not as a paste.
    pub fn press(&self, bytes: &[u8]) {
        self.send(bytes);
    }

    fn send(&self, bytes: &[u8]) {
        self.typed_at.set(Some(Instant::now()));
        let mut g = self.grid.borrow_mut();
        if let Some(ch) = g.channel
            && !bytes.is_empty()
        {
            g.vt.scroll_to_bottom();
            g.core.input(ch, bytes);
        }
    }

    fn paste_paths(&self, paths: impl IntoIterator<Item = String>) {
        match drag_paths::quote_paths(paths, &self.address, cfg!(windows)) {
            Ok(text) => {
                let bytes = self.grid.borrow().vt.paste_bytes(&text);
                self.send(&bytes);
            }
            Err(body) => drag_paths::unavailable(body),
        }
    }

    fn dropped(&mut self, paths: Vec<std::path::PathBuf>, cx: &mut Context<Self>) {
        if !self.address.contains('/') {
            let paths = stage_temporary_screenshot(&paths).unwrap_or(paths);
            return self.paste_paths(paths.iter().map(|p| p.to_string_lossy().into_owned()));
        }
        self.put_there(paths, false, cx);
    }

    /// Copies files to this terminal's machine (`hc put`) and pastes their
    /// paths there; `copies`: the local files were only for sending.
    fn put_there(&self, paths: Vec<std::path::PathBuf>, copies: bool, cx: &mut Context<Self>) {
        if let Err(body) = drag_paths::validate_upload(&paths) {
            return drag_paths::unavailable(body);
        }
        let addr = self.address.clone();
        static SENDS: AtomicU64 = AtomicU64::new(0);
        let send = SENDS.fetch_add(1, Ordering::Relaxed);
        let label = match (copies, paths.len()) {
            (true, _) => "Sending image…",
            (false, 1) => "Sending file…",
            _ => "Sending files…",
        };
        let done = Rc::new(Cell::new(false));
        let shown = done.clone();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            if !shown.get() {
                let _ = this.update(cx, |this, cx| {
                    this.sending.push((send, label));
                    cx.notify();
                });
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let (there, refused) = cx
                .background_executor()
                .spawn(async move {
                    let (mut there, mut refused) = (Vec::new(), Vec::new());
                    for p in &paths {
                        let name = || if copies { "the pasted image".to_owned() } else { p.file_name().unwrap_or(p.as_os_str()).to_string_lossy().into_owned() };
                        if let Some(why) = drag_paths::upload_refusal(p) {
                            refused.push((name(), why.to_owned()));
                            continue;
                        }
                        let out = crate::spawn::command(crate::core_bin::path()).args(["put", "--json", &addr]).arg(p).output();
                        if copies {
                            let _ = std::fs::remove_file(p);
                        }
                        match out.map_err(|e| e.to_string()).and_then(|o| put_answer(&o.stdout, &o.stderr)) {
                            Ok(path) => there.push(path),
                            Err(why) => refused.push((name(), why)),
                        }
                    }
                    (there, refused)
                })
                .await;
            // A closed pane is gone here: its paths go nowhere.
            let _ = this.update(cx, |this, cx| {
                done.set(true);
                this.sending.retain(|(s, _)| *s != send);
                cx.notify();
                if !there.is_empty() {
                    this.paste_paths(there);
                }
                if let Some((title, body)) = refusal_notice(&refused) {
                    cx.emit(TerminalEvent::Notice { title, body });
                }
            });
        })
        .detach();
    }

    /// Text as a paste; copied files (Finder, a clipboard manager's image) as
    /// if dropped; an image alone (a screenshot) as Ctrl+V pastes it.
    pub fn paste(&mut self, cx: &mut Context<Self>) {
        let Some(item) = cx.read_from_clipboard() else { return };
        if let Some(paths) = copied_files(&item) {
            self.dropped(paths, cx);
        } else if let Some(text) = item.text() {
            let bytes = self.grid.borrow().vt.paste_bytes(&text);
            self.send(&bytes);
        } else if item.entries().iter().any(|e| matches!(e, ClipboardEntry::Image(_))) {
            self.paste_image(cx);
        }
    }

    /// Ctrl+V: a remote image, clipboard text on Windows, or the key itself.
    pub fn paste_image(&mut self, cx: &mut Context<Self>) {
        if self.send_image(cx) {
            return;
        }
        let text = if cfg!(windows) { cx.read_from_clipboard().and_then(|c| c.text()) } else { None };
        let bytes = paste_image_fallback(&mut self.grid.borrow_mut().vt, text.as_deref());
        if let Some(bytes) = bytes {
            self.send(&bytes);
        }
    }

    /// An agent on another machine reads that machine's clipboard, so this
    /// one's image goes there as a file, and its path is pasted. A local
    /// program reads the clipboard itself: nothing to do.
    fn send_image(&self, cx: &mut Context<Self>) -> bool {
        if !self.address.contains('/') {
            return false;
        }
        let Some(path) = cx.read_from_clipboard().and_then(|item| image_file(&item)) else { return false };
        self.put_there(vec![path], true, cx);
        true
    }

    /// Copies the selection to the clipboard (and the primary selection on Linux).
    /// Returns whether anything was copied. Does not clear the selection: mouse
    /// copy-on-select uses this and keeps the highlight.
    pub fn copy(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(text) = self.grid.borrow().vt.selection_text() else {
            return false;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
        #[cfg(target_os = "linux")]
        cx.write_to_primary(ClipboardItem::new_string(text));
        true
    }

    fn copy_primary(&self, _cx: &mut Context<Self>) {
        #[cfg(target_os = "linux")]
        if let Some(text) = self.grid.borrow().vt.selection_text() {
            _cx.write_to_primary(ClipboardItem::new_string(text));
        }
    }

    pub fn selection(&self) -> Option<vt::SelectionPositions> {
        self.grid.borrow().vt.selection_positions()
    }

    fn selection_changed(&mut self, cx: &mut Context<Self>) {
        let selection = self.selection();
        if selection != self.published_selection {
            self.published_selection = selection;
            cx.emit(TerminalEvent::SelectionChanged);
        }
    }

    pub fn selection_command(&mut self, name: &str, args: &serde_json::Value, cx: &mut Context<Self>) -> Result<(), String> {
        let mut grid = self.grid.borrow_mut();
        match name {
            "terminal.select" => {
                let at = match args.get("at") {
                    None => None,
                    Some(value) if value.as_str() == Some("cursor") => None,
                    Some(value) => Some(serde_json::from_value::<vt::CellPosition>(value.clone())
                        .map_err(|_| "terminal.select at is cursor or {row, col} with nonnegative whole numbers")?),
                };
                let mode = match args.get("mode") {
                    None => "cell",
                    Some(value) => value.as_str().ok_or("terminal.select mode is cell, word or line")?,
                };
                grid.vt.select_at(at, mode)?;
            }
            "terminal.adjust_selection" => {
                let direction = args.get("direction").and_then(serde_json::Value::as_str).ok_or("terminal.adjust_selection needs a direction")?;
                grid.vt.adjust_selection(direction)?;
            }
            "terminal.clear_selection" => grid.vt.clear_selection(),
            _ => unreachable!(),
        }
        drop(grid);
        self.selection_changed(cx);
        self.publish_scroll_if_changed(cx);
        self.copy_primary(cx);
        cx.notify();
        Ok(())
    }

    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        self.grid.borrow_mut().vt.select_all();
        self.selection_changed(cx);
        self.copy_primary(cx);
        cx.notify();
    }

    pub fn clear_selection(&mut self, cx: &mut Context<Self>) {
        self.grid.borrow_mut().vt.clear_selection();
        self.selection_changed(cx);
        cx.notify();
    }

    /// Opens the find bar inside this terminal view. Keeping the input entity
    /// here means its query and selection survive tab switches independently.
    pub fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let reopening = !self.search_open;
        let input = if let Some(input) = self.search_input.clone() {
            input
        } else {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder("Find in terminal"));
            cx.subscribe(&input, |this, input, event: &InputEvent, cx| match event {
                InputEvent::Change => {
                    this.search_query = input.read(cx).value().to_string();
                    this.pending_search_nav = None;
                    this.grid.borrow_mut().vt.set_search_needle(&this.search_query);
                    this.ensure_search_task(cx);
                    cx.notify();
                }
                InputEvent::PressEnter { shift, .. } => this.queue_search_navigation(*shift, cx),
                InputEvent::Focus => this.search_keys = true,
                _ => {}
            })
            .detach();
            self.search_input = Some(input.clone());
            input
        };
        // Re-applied on every open, so a theme change while the bar was closed
        // doesn't leave the field with the old colors.
        input.update(cx, |state, _| state.set_editor_style(find_input_style()));

        self.search_open = true;
        self.search_keys = true;
        self.search_query = input.read(cx).value().to_string();
        self.grid.borrow_mut().vt.start_search(&self.search_query);
        self.focus_search_input(window, cx);
        if reopening && !self.search_query.is_empty() {
            if let Some(input) = &self.search_input {
                input.update(cx, |state, cx| state.select_all(window, cx));
            }
        }
        self.ensure_search_task(cx);
        cx.notify();
    }

    fn focus_search_input(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(input) = &self.search_input {
            input.update(cx, |state, cx| state.focus(window, cx));
        }
    }

    fn close_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search_open = false;
        self.pending_search_nav = None;
        self.search_task.take();
        self.grid.borrow_mut().vt.clear_search();
        self.focus.focus(window, cx);
        cx.notify();
    }

    fn queue_search_navigation(&mut self, previous: bool, cx: &mut Context<Self>) {
        if !self.search_open || self.search_query.is_empty() {
            return;
        }
        if self.grid.borrow().vt.search_progress().complete {
            self.pending_search_nav = None;
            if self.grid.borrow_mut().vt.search_navigate(previous) {
                self.publish_scroll_if_changed(cx);
                cx.notify();
            }
        } else {
            self.pending_search_nav = Some(previous);
            self.ensure_search_task(cx);
        }
    }

    fn ensure_search_task(&mut self, cx: &mut Context<Self>) {
        if !self.search_open || self.search_query.is_empty() || self.search_task.is_some() {
            return;
        }
        self.search_task = Some(cx.spawn(async move |this, cx| loop {
            let delay = this.update(cx, |this, cx| {
                if !this.search_open {
                    return None;
                }
                if this.shown_in.get() != FRAME.load(Ordering::Relaxed) {
                    return Some(Duration::from_millis(250));
                }
                if this.search_query.is_empty() {
                    return Some(Duration::from_millis(200));
                }

                let mut grid = this.grid.borrow_mut();
                let mut changed = grid.vt.progress_search();
                if grid.vt.search_progress().complete
                    && let Some(previous) = this.pending_search_nav.take()
                {
                    changed |= grid.vt.search_navigate(previous);
                }
                let busy = !grid.vt.search_progress().complete || this.pending_search_nav.is_some();
                drop(grid);
                if changed {
                    this.publish_scroll_if_changed(cx);
                    cx.notify();
                }
                Some(if busy { Duration::from_millis(33) } else { Duration::from_millis(200) })
            }).unwrap_or(None);
            let Some(delay) = delay else { break };
            cx.background_executor().timer(delay).await;
        }));
    }

    fn key_down(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.find_field_focused(window, cx) && ev.keystroke.key == "escape" {
            self.close_search(window, cx);
            cx.stop_propagation();
            return;
        }
        if self.find_field_focused(window, cx) {
            // The input entity owns text editing and Enter navigation; never
            // leak its keystrokes into the shell running in this terminal.
            return;
        }
        let k = &ev.keystroke;
        // Typing clears a selection, as in Herdr.
        if !k.key.starts_with("shift") && !k.key.starts_with("control") && !k.key.starts_with("alt") {
            self.clear_selection(cx);
        }
        let bytes = key_bytes(&mut self.grid.borrow_mut().vt, ev);
        if let Some(bytes) = bytes {
            self.send(&bytes);
            cx.stop_propagation();
        }
    }

    fn scroll(&mut self, ev: &ScrollWheelEvent, _w: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        let mut g = self.grid.borrow_mut();
        let steps = scroll_delta_steps(&mut self.scrolled, ev.delta, ev.touch_phase, g.cell, g.vt.cols, g.vt.rows, crate::settings::get().terminal_scroll_speed);
        let n = steps.y.unsigned_abs() as usize;
        if steps.x == 0 && n == 0 {
            return;
        }
        let up = steps.y > 0;
        if g.vt.mouse_tracking() {
            let ((x, y), size) = mouse_at(&g, ev.position);
            let out = scroll_mouse_bytes(&mut g.vt, steps, ev.modifiers, x, y, size);
            if let Some(ch) = g.channel {
                g.core.input(ch, &out);
            }
        } else if n == 0 {
            return;
        } else if g.vt.alt_screen() {
            let key = if up { GHOSTTY_KEY_ARROW_UP } else { GHOSTTY_KEY_ARROW_DOWN };
            let mut out = Vec::new();
            for _ in 0..n {
                out.extend(g.vt.encode_key(key, 0, 0, None, None));
            }
            if let Some(ch) = g.channel {
                g.core.input(ch, &out);
            }
        } else {
            g.vt.scroll_viewport(if up { -(n as isize) } else { n as isize });
        }
        drop(g);
        self.publish_scroll_if_changed(cx);
        cx.notify();
    }

    pub fn scroll_history(&mut self, target: ScrollTarget, cx: &mut Context<Self>) {
        let mut g = self.grid.borrow_mut();
        let rows = g.vt.rows;

        match target {
            ScrollTarget::Lines(lines) => g.vt.scroll_viewport(lines),
            ScrollTarget::Pages(pages) => g.vt.scroll_viewport(pages_to_lines(pages, rows)),
            ScrollTarget::Top => g.vt.scroll_to_top(),
            ScrollTarget::Bottom => g.vt.scroll_to_bottom(),
        }
        drop(g);
        self.publish_scroll_if_changed(cx);
        cx.notify();
    }

    /// History scroll metrics for mods (`panes().scroll`). None on the alternate screen.
    pub fn scroll_metrics(&self) -> Option<ScrollMetrics> {
        let g = self.grid.borrow();
        if g.vt.alt_screen() {
            return None;
        }
        let sb = g.vt.scrollbar();
        let from_bottom = sb.total.saturating_sub(sb.offset.saturating_add(sb.len));
        Some(ScrollMetrics { total: sb.total, offset: sb.offset, len: sb.len, from_bottom })
    }

    fn publish_scroll_if_changed(&mut self, cx: &mut Context<Self>) {
        let from_bottom = self.scroll_metrics().map(|m| m.from_bottom);
        let previous = self.published_from_bottom;
        self.published_from_bottom = from_bottom;
        if scroll_state_changed(previous, from_bottom) {
            cx.emit(TerminalEvent::ScrollChanged);
        }
    }

    /// The program tracks the mouse and Shift isn't held (Shift keeps the mouse Herdr's).
    fn takes_mouse(&self, mods: Modifiers) -> bool {
        self.grid.borrow().vt.mouse_tracking() && !mods.shift
    }

    /// Sends a mouse event to a program that tracks the mouse. Returns whether it did
    /// (shift keeps the mouse for Herdr: selecting, the menu).
    fn mouse(&mut self, action: GhosttyMouseAction, button: Option<MouseButton>, pos: Point<Pixels>, mods: Modifiers) -> bool {
        let mut g = self.grid.borrow_mut();
        if !track_mouse_button(&mut g.buttons_down, action, button) {
            return false;
        }
        if !g.vt.mouse_tracking() || mods.shift {
            return false;
        }
        let b = button.map(|b| match b {
            MouseButton::Left => GHOSTTY_MOUSE_BUTTON_LEFT,
            MouseButton::Right => GHOSTTY_MOUSE_BUTTON_RIGHT,
            MouseButton::Middle => GHOSTTY_MOUSE_BUTTON_MIDDLE,
            _ => GHOSTTY_MOUSE_BUTTON_UNKNOWN,
        });
        let ((x, y), size) = mouse_at(&g, pos);
        let any = !g.buttons_down.is_empty();
        let bytes = g.vt.encode_mouse(action, b, mouse_mods(mods), x, y, size, any);
        if let Some(ch) = g.channel {
            g.core.input(ch, &bytes);
        }
        true
    }

    /// The pointer is over the pane: pressing or letting go of the link key redraws it.
    pub fn hovered(&self) -> bool {
        self.grid.borrow().hover.is_some()
    }

    /// The link at a window position, and where it shows.
    fn link_at(&self, pos: Point<Pixels>) -> Option<(LinkTarget, vt::Spans)> {
        let g = self.grid.borrow();
        let (x, y) = local(&g, pos);
        let (col, row) = (x / f32::from(g.cell.width), y / f32::from(g.cell.height));
        let mut target = None;
        let hit = g.vt.link_at(col as u16, row as u16, |text, at, joined| {
            // In lines joined back together only a file that exists proves the join right.
            if let Some(range) = links::find(text, at)
                && let Some(link) = links::resolve(&text[range.clone()], self.cwd.as_deref())
                && (!joined || matches!(link, links::Target::File(_)))
            {
                target = Some(LinkTarget::External(link));
                return Some(range);
            }
            if joined {
                return None;
            }
            None
        })?;
        // OSC 8 links bypass text detection.
        let target = target.or_else(|| links::resolve(&hit.target, self.cwd.as_deref()).map(LinkTarget::External))?;
        Some((target, hit.spans))
    }

    fn start_selection(&mut self, pos: Point<Pixels>, cx: &mut Context<Self>) {
        self.selecting = true;
        self.selection_pointer = pos;
        self.select(Select::Press, pos, cx);
        self.selection_scroll = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(50)).await;
                let keep = this.update(cx, |this, cx| {
                    if !this.selecting { return false; }
                    if this.shown_in.get() != FRAME.load(Ordering::Relaxed) {
                        this.finish_selection(None, cx);
                        return false;
                    }
                    let mut g = this.grid.borrow_mut();
                    let y = f32::from(this.selection_pointer.y - g.origin.y);
                    let lines = selection_scroll_lines(y, f32::from(g.cell.height), g.vt.rows);
                    if lines != 0 {
                        g.vt.scroll_viewport(lines);
                        drop(g);
                        this.select(Select::Drag, this.selection_pointer, cx);
                        this.publish_scroll_if_changed(cx);
                    }
                    true
                }).unwrap_or(false);
                if !keep { break; }
            }
        }));
    }

    fn finish_selection(&mut self, pos: Option<Point<Pixels>>, cx: &mut Context<Self>) {
        self.selecting = false;
        let mut g = self.grid.borrow_mut();
        g.buttons_down.retain(|b| *b != MouseButton::Left);
        if let Some(pos) = pos {
            drop(g);
            self.select(Select::Release, pos, cx);
        } else {
            g.vt.clear_selection();
            drop(g);
            self.selection_changed(cx);
            cx.notify();
        }
    }

    /// Selecting with the mouse (when the program doesn't take it).
    fn select(&mut self, step: Select, pos: Point<Pixels>, cx: &mut Context<Self>) {
        let mut g = self.grid.borrow_mut();
        let (x, y) = local(&g, pos);
        let (cw, ch) = (f32::from(g.cell.width), f32::from(g.cell.height));
        let (col, row) = ((x / cw) as u16, (y / ch) as u16);
        let height = ch * g.vt.rows as f32;
        g.vt.select(step, col, row, x, y, cw, height);
        drop(g);
        self.selection_changed(cx);
        if matches!(step, Select::Release) {
            if crate::settings::get().copy_on_select {
                // Copy on select, as in Herdr.
                self.copy(cx);
            } else {
                self.copy_primary(cx);
            }
        }
        cx.notify();
    }
}

#[derive(Debug, PartialEq)]
pub enum ScrollTarget {
    Lines(isize),
    Pages(f64),
    Top,
    Bottom,
}

pub fn scroll_target(args: &Value) -> Result<ScrollTarget, String> {
    let given: Vec<&str> = ["lines", "pages", "to"].into_iter().filter(|key| is_given(args, key)).collect();
    match given.as_slice() {
        [] => Err("terminal.scroll needs one of lines, pages, to".into()),
        ["lines"] => whole_lines_arg(args).map(ScrollTarget::Lines),
        ["pages"] => pages_arg(args).map(ScrollTarget::Pages),
        ["to"] => edge_arg(args),
        _ => Err(format!("terminal.scroll takes one of lines, pages, to, not {}", given.join(" and "))),
    }
}

fn is_given(args: &Value, key: &str) -> bool {
    args.get(key).is_some_and(|value| !value.is_null())
}

fn whole_lines_arg(args: &Value) -> Result<isize, String> {
    let value = &args["lines"];
    value.as_i64().and_then(|lines| isize::try_from(lines).ok()).ok_or_else(|| format!("terminal.scroll lines is not a whole number: {value}"))
}

fn pages_arg(args: &Value) -> Result<f64, String> {
    let value = &args["pages"];
    value.as_f64().filter(|pages| pages.is_finite()).ok_or_else(|| format!("terminal.scroll pages is not a finite number: {value}"))
}

fn edge_arg(args: &Value) -> Result<ScrollTarget, String> {
    let value = &args["to"];
    match value.as_str() {
        Some("top") => Ok(ScrollTarget::Top),
        Some("bottom") => Ok(ScrollTarget::Bottom),
        _ => Err(format!("terminal.scroll to is \"top\" or \"bottom\", not {value}")),
    }
}

fn pages_to_lines(pages: f64, rows: u16) -> isize {
    (pages * f64::from(rows)).round() as isize
}

fn selection_scroll_lines(y: f32, cell_height: f32, rows: u16) -> isize {
    let height = cell_height * rows as f32;
    let outside = if y < 0.0 { y } else if y >= height { y - height + 1.0 } else { return 0 };
    (outside / cell_height).abs().ceil().clamp(1.0, 5.0) as isize * if outside < 0.0 { -1 } else { 1 }
}

/// A scroll thumb's top and length in px inside a `track` px lane: the
/// scrollable area is `total` rows, the `len`-row viewport sits `offset` rows
/// from its top. None when nothing scrolls; `min` keeps a huge scrollback
/// grabbable. The proportions are Herdr's scrollbar's.
fn scrollbar_thumb(total: u64, offset: u64, len: u64, track: f32, min: f32) -> Option<(f32, f32)> {
    if total <= len || len == 0 || track <= 0.0 {
        return None;
    }
    let thumb = (track * len as f32 / total as f32).clamp(min.min(track), track);
    let travel = track - thumb;
    let top = if travel <= 0.0 { 0.0 } else { offset.min(total - len) as f32 / (total - len) as f32 * travel };
    Some((top, thumb))
}

// Use existing padding when possible, otherwise overlay the last few pixels.
// Scrollback appearing must not resize the terminal or reflow its output.
fn scrollbar_lane(bounds: Bounds<Pixels>, origin: Point<Pixels>, text_width: Pixels, height: Pixels) -> Bounds<Pixels> {
    let edge = bounds.origin.x + bounds.size.width;
    let left = (origin.x + text_width).min(edge - px(8.0)).max(bounds.origin.x);
    Bounds::new(point(left, origin.y), size(edge - left, height))
}

fn scrollbar_grab_at(top: f32, len: f32, y: f32) -> f32 {
    if y >= top && y < top + len { y - top } else { len / 2.0 }
}

/// The scroll position a thumb dragged to `top` px lands on, as a row offset
/// from the scrollback's top (`Vt::scroll_to_row`'s).
fn scrollbar_row_at(total: u64, len: u64, track: f32, min: f32, top: f32) -> usize {
    let Some((_, thumb)) = scrollbar_thumb(total, 0, len, track, min) else { return 0 };
    let travel = track - thumb;
    if travel <= 0.0 {
        return 0;
    }
    (top.clamp(0.0, travel) / travel * (total - len) as f32).round() as usize
}

/// A title as it's shown: an agent's spinner in front of it (`\u{2733} Fix the
/// tests`, braille dots) is no part of it, nor braille dots after a short
/// prefix (`\u{03c0} \u{2839} task`). Herdr draws its own, and a title that
/// spins would redraw every mod that shows titles, 10 times a second.
pub(crate) fn shown_title(title: &str) -> std::borrow::Cow<'_, str> {
    use std::borrow::Cow;
    // Frames agents spin, not status icons (\u{2713} build stays): braille dots,
    // Claude Code's stars and its half circles.
    let spinner = |c: char| {
        matches!(c, '\u{2800}'..='\u{28ff}' | '\u{00b7}' | '\u{2722}' | '\u{2733}' | '\u{2736}' | '\u{273b}' | '\u{273d}' | '\u{25d0}'..='\u{25d3}')
    };
    let rest = title.trim_start_matches(|c: char| spinner(c) || c.is_whitespace());
    let led = title.len() - rest.len();
    if !rest.is_empty() && title[..led].chars().any(spinner) && title[..led].ends_with(char::is_whitespace) {
        return Cow::Borrowed(rest);
    }
    let braille = |w: &str| !w.is_empty() && w.chars().all(|c| matches!(c, '\u{2800}'..='\u{28ff}'));
    let mut words = title.splitn(3, ' ');
    match (words.next(), words.next()) {
        (Some(prefix), Some(frame)) if !prefix.is_empty() && prefix.chars().count() <= 3 && !braille(prefix) && braille(frame) => {
            match words.next().map(str::trim_start).filter(|r| !r.is_empty()) {
                Some(rest) => Cow::Owned(format!("{prefix} {rest}")),
                None => Cow::Borrowed(prefix),
            }
        }
        _ => Cow::Borrowed(title),
    }
}

/// A live macOS screenshot thumbnail, dropped before it saved.
///
/// macOS writes such an image into a private `NSIRD_*` directory under
/// `TemporaryItems` and grants read access to it only to the app that received
/// the drop, and only until the thumbnail saves.
fn is_temporary_screenshot(path: &Path) -> bool {
    if !path.is_absolute() {
        return false;
    }
    path.ancestors().skip(1).any(|directory| {
        directory
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("NSIRD_"))
            && directory
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|name| name == "TemporaryItems")
    })
}

/// Whether these bytes are an image the pane can open.
fn is_image_bytes(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\x89PNG\r\n\x1a\n") || bytes.starts_with(&[0xff, 0xd8, 0xff])
}

/// Copies a live screenshot thumbnail somewhere a pane can read, and pastes
/// that path instead.
///
/// macOS gives read access to the thumbnail's temporary file only to the app
/// that received the drop; a pane's processes are children of the Herdr
/// daemon, not of this app, so they cannot open it, and the file is gone
/// within seconds. This app can read it now, so it copies the bytes into the
/// temp directory and the pane gets a path that stays readable.
fn stage_temporary_screenshot(paths: &[PathBuf]) -> Option<Vec<PathBuf>> {
    let [path] = paths else {
        return None;
    };
    if !is_temporary_screenshot(path) {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    if !is_image_bytes(&bytes) {
        return None;
    }
    let directory = std::env::temp_dir().join("herdr-drops");
    std::fs::create_dir_all(&directory).ok()?;
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok()?.as_millis();
    let name = path.file_name()?.to_str()?;
    let staged = directory.join(format!("{stamp}-{name}"));
    std::fs::write(&staged, &bytes).ok()?;
    Some(vec![staged])
}

/// Files on the clipboard: their text is only the bare path, which an agent
/// reads as words, not as an image to attach.
fn copied_files(item: &ClipboardItem) -> Option<Vec<std::path::PathBuf>> {
    item.entries().iter().find_map(|e| match e {
        ClipboardEntry::ExternalPaths(p) if !p.paths().is_empty() => Some(p.paths().to_vec()),
        _ => None,
    })
}

/// The clipboard's image saved to a file of its own, for its path to be pasted.
/// `hc put --json`'s answer: the path there, or why it refused (on stderr).
fn put_answer(stdout: &[u8], stderr: &[u8]) -> Result<String, String> {
    let json = |b: &[u8]| serde_json::from_slice::<serde_json::Value>(b).unwrap_or_default();
    if let Some(path) = json(stdout)["path"].as_str() {
        return Ok(path.to_owned());
    }
    if let Some(why) = json(stderr)["error"]["message"].as_str() {
        return Err(why.to_owned());
    }
    let text = String::from_utf8_lossy(stderr).trim().to_owned();
    Err(if text.is_empty() { "hc put gave no path".to_owned() } else { text })
}

fn refusal_notice(refused: &[(String, String)]) -> Option<(String, String)> {
    match refused {
        [] => None,
        [(name, why)] => Some((format!("Couldn't send {name}"), why.clone())),
        many => {
            let mut why: Vec<&str> = many.iter().map(|(_, w)| w.as_str()).collect();
            why.dedup();
            Some((format!("Couldn't send {} files", many.len()), why.join("; ")))
        }
    }
}

fn image_file(item: &ClipboardItem) -> Option<std::path::PathBuf> {
    let image = item.entries().iter().find_map(|e| match e {
        ClipboardEntry::Image(i) => Some(i),
        _ => None,
    })?;
    let (format, bytes) = paste_image_bytes(image.format, &image.bytes)?;
    let ext = match format {
        ImageFormat::Jpeg => "jpg",
        ImageFormat::Gif => "gif",
        ImageFormat::Webp => "webp",
        ImageFormat::Svg => "svg",
        _ => "png",
    };
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos();
    let path = std::env::temp_dir().join(format!("herdr-paste-{stamp}.{ext}"));
    std::fs::write(&path, bytes).ok()?;
    Some(path)
}

fn paste_image_bytes(format: ImageFormat, bytes: &[u8]) -> Option<(ImageFormat, std::borrow::Cow<'_, [u8]>)> {
    let source = match format {
        ImageFormat::Tiff => image::ImageFormat::Tiff,
        ImageFormat::Bmp => image::ImageFormat::Bmp,
        _ => return Some((format, std::borrow::Cow::Borrowed(bytes))),
    };
    let image = image::load_from_memory_with_format(bytes, source).ok()?;
    let mut png = std::io::Cursor::new(Vec::new());
    image.write_to(&mut png, image::ImageFormat::Png).ok()?;
    Some((ImageFormat::Png, std::borrow::Cow::Owned(png.into_inner())))
}

fn mouse_mods(m: Modifiers) -> GhosttyMods {
    let mut mods = 0;
    if m.shift {
        mods |= GHOSTTY_MODS_SHIFT as GhosttyMods;
    }
    if m.alt {
        mods |= GHOSTTY_MODS_ALT as GhosttyMods;
    }
    if m.control {
        mods |= GHOSTTY_MODS_CTRL as GhosttyMods;
    }
    mods
}

fn link_mods(m: &Modifiers) -> bool {
    let key = if cfg!(target_os = "macos") { m.platform && !m.control } else { m.control && !m.platform };
    key && !m.alt && !m.shift
}

fn local(g: &Grid, p: Point<Pixels>) -> (f32, f32) {
    (f32::from(p.x - g.origin.x).max(0.0), f32::from(p.y - g.origin.y).max(0.0))
}

fn scroll_delta_steps(scrolled: &mut Point<f32>, delta: ScrollDelta, phase: TouchPhase, cell: Size<Pixels>, cols: u16, rows: u16, speed: f32) -> Point<i32> {
    let lines = match delta {
        // As in Ghostty, a sideways notch moves one column; a vertical one, three lines.
        ScrollDelta::Lines(p) if cfg!(target_os = "macos") => {
            if speed == 1.0 {
                return point(macos_wheel_steps(p.x) / 3, macos_wheel_steps(p.y));
            }
            let y = if p.y == 0.0 { 0.0 } else { p.y.signum() * p.y.abs().max(1.0) * 3.0 * speed };
            return point(macos_wheel_steps(p.x) / 3, scroll_steps(&mut scrolled.y, phase, y, rows as f32));
        },
        ScrollDelta::Lines(p) => p,
        ScrollDelta::Pixels(p) => point(p.x / cell.width, p.y / cell.height),
    };
    point(scroll_steps(&mut scrolled.x, phase, lines.x, cols as f32), scroll_steps(&mut scrolled.y, phase, lines.y * speed, rows as f32))
}

fn scroll_mouse_bytes(vt: &mut Vt, steps: Point<i32>, mods: Modifiers, x: f32, y: f32, size: (u32, u32, f32, f32)) -> Vec<u8> {
    let mut out = Vec::new();
    for (steps, positive, negative) in [
        (steps.y, GHOSTTY_MOUSE_BUTTON_FOUR, GHOSTTY_MOUSE_BUTTON_FIVE),
        (steps.x, GHOSTTY_MOUSE_BUTTON_SIX, GHOSTTY_MOUSE_BUTTON_SEVEN),
    ] {
        let button = if steps > 0 { positive } else { negative };
        for _ in 0..steps.unsigned_abs() {
            out.extend(vt.encode_mouse(GHOSTTY_MOUSE_ACTION_PRESS, Some(button), mouse_mods(mods), x, y, size, false));
        }
    }
    out
}

/// Whole lines to scroll for one scroll event. A touchpad sends many small
/// moves: they add up to a line before one scrolls (as in Zed), and each
/// gesture starts from nothing. Kept under a screen, so turning back is quick.
fn scroll_steps(scrolled: &mut f32, phase: TouchPhase, lines: f32, rows: f32) -> i32 {
    match phase {
        TouchPhase::Started => {
            *scrolled = 0.0;
            0
        }
        TouchPhase::Moved => {
            let before = scrolled.trunc() as i32;
            *scrolled += lines;
            let after = scrolled.trunc() as i32;
            *scrolled %= rows.max(1.0);
            after - before
        }
        _ => 0,
    }
}

/// Lines for a macOS mouse wheel event, as Ghostty scrolls them. macOS reports
/// wheel ticks, a slow notch as a fraction of one (often 0.1), so an event counts
/// as at least one tick, times Ghostty's default wheel multiplier of 3, with the
/// fraction dropped.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn macos_wheel_steps(ticks: f32) -> i32 {
    if ticks == 0.0 {
        return 0;
    }
    (ticks.signum() * ticks.abs().max(1.0) * 3.0).trunc() as i32
}

/// A pointer position and the grid for mouse reports, in device pixels
/// (SGR-pixels mode reports pixels, as in Ghostty).
fn mouse_at(g: &Grid, p: Point<Pixels>) -> ((f32, f32), (u32, u32, f32, f32)) {
    let ((x, y), s) = (local(g, p), g.scale);
    ((x * s, y * s), (g.vt.cols as u32, g.vt.rows as u32, f32::from(g.cell.width) * s, f32::from(g.cell.height) * s))
}

/// An icon's glyph in the bundled symbols font, with its outline's bounds in
/// font units (y up).
struct IconGlyph {
    id: u32,
    units_per_em: f32,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
}

/// Read from the font file itself, not through text shaping: shaping may
/// take the character from another installed font (old Nerd Fonts put a
/// squirrel where the markdown icon is now).
fn icon_glyph(c: char) -> Option<IconGlyph> {
    thread_local! {
        static FACE: Option<ttf_parser::Face<'static>> = ttf_parser::Face::parse(crate::settings::SYMBOLS_FONT_FILE, 0).ok();
    }
    FACE.with(|face| {
        let face = face.as_ref()?;
        let id = face.glyph_index(c)?;
        let b = face.glyph_bounding_box(id)?;
        Some(IconGlyph { id: id.0 as u32, units_per_em: face.units_per_em() as f32, x0: b.x_min as f32, y0: b.y_min as f32, x1: b.x_max as f32, y1: b.y_max as f32 })
    })
}

/// A Nerd Font icon (Unicode private use areas).
fn is_icon(text: &str) -> bool {
    let mut chars = text.chars();
    matches!((chars.next(), chars.next()), (Some(c), None) if matches!(c as u32, 0xE000..=0xF8FF | 0xF0000..=0x10FFFD))
}

/// Keep the configured font’s icon proportions; only fit bundled fallback glyphs.
fn fallback_icon(text: &str, font: &Font, ts: &gpui::TextSystem) -> Option<IconGlyph> {
    let c = text.chars().next().filter(|_| is_icon(text))?;
    let id = ts.resolve_font(font);
    if ts.advance(id, px(crate::settings::font_size()), c).is_ok() {
        return None;
    }
    icon_glyph(c)
}

fn paste_image_fallback(vt: &mut vt::Vt, text: Option<&str>) -> Option<Vec<u8>> {
    // Windows console programs expect the terminal to paste clipboard text.
    if cfg!(windows) && let Some(text) = text {
        return Some(vt.paste_bytes(text));
    }
    let keystroke = Keystroke::parse("ctrl-v").ok()?;
    key_bytes(vt, &KeyDownEvent { keystroke, is_held: false, prefer_character_input: false })
}

/// What a key press sends to the program: the key described to libghostty,
/// which encodes it once, in the mode the program asked for (as Ghostty does).
fn key_bytes(vt: &mut vt::Vt, ev: &KeyDownEvent) -> Option<Vec<u8>> {
    // GPUI sets this preference on replayed shortcut prefixes too; only
    // Windows' native composition previews defer to the text input handler.
    #[cfg(windows)]
    if ev.prefer_character_input && gpui_windows::is_native_character_input(ev) {
        return None;
    }
    let k = &ev.keystroke;
    let m = k.modifiers;
    // libghostty leaves these aliases to fixterms, but GPUI drops their
    // control text. Legacy needs explicit aliases; negotiated modes keep the chord.
    if !vt.win32.on && vt.kitty_keyboard_flags() == 0 && m.control && !m.alt && !m.platform && !m.function {
        let alias = match k.key.as_str() {
            "[" if !m.shift => Some((GHOSTTY_KEY_BRACKET_LEFT, GHOSTTY_KEY_ESCAPE)),
            "i" if !m.shift => Some((GHOSTTY_KEY_I, GHOSTTY_KEY_TAB)),
            "m" if !m.shift => Some((GHOSTTY_KEY_M, GHOSTTY_KEY_ENTER)),
            _ => None,
        };
        if let Some((key, alias)) = alias {
            let legacy = vt.encode_key(alias, 0, 0, None, None);
            return Some(control_alias_bytes(vt, key, &k.key, false, legacy));
        }
        if matches!(k.key.as_str(), "-" | "_") {
            let shifted = m.shift || k.key == "_";
            let text = if shifted { "_" } else { "-" };
            return Some(control_alias_bytes(vt, GHOSTTY_KEY_MINUS, text, shifted, vec![0x1f]));
        }
    }
    if vt.win32.on && vt.kitty_keyboard_flags() == 0 {
        if k.key == "enter" && (m.shift || m.control || m.alt) {
            return Some(crate::win32_keys::enter(m.shift, m.control, m.alt));
        }
        // ConPTY combines a bare Escape with the next key as an Alt chord.
        if k.key == "escape" {
            return Some(crate::win32_keys::escape(m.shift, m.control, m.alt));
        }
    }
    let Some((key, shifted, unshifted)) = map_key(&k.key) else {
        return k.key_char.as_ref().map(|text| text.as_bytes().to_vec());
    };
    let mut mods: GhosttyMods = 0;
    if m.shift || shifted {
        mods |= GHOSTTY_MODS_SHIFT as GhosttyMods;
    }
    if m.control {
        mods |= GHOSTTY_MODS_CTRL as GhosttyMods;
    }
    if m.alt {
        mods |= GHOSTTY_MODS_ALT as GhosttyMods;
        #[cfg(target_os = "macos")]
        if k.option_side == Some(gpui::OptionSide::Right)
            || (k.option_side == Some(gpui::OptionSide::Both) && gpui_macos::option_as_alt() == gpui_macos::OptionAsAlt::Right)
        {
            mods |= GHOSTTY_MODS_ALT_SIDE as GhosttyMods;
        }
    }
    if m.platform {
        mods |= GHOSTTY_MODS_SUPER as GhosttyMods;
    }
    let text = match k.key.as_str() {
        "space" => Some(" ".to_owned()),
        _ => k.key_char.clone(),
    };
    // macOS translates Option into text unless it is configured as Alt.
    // That text can be ASCII too (Slovenian Shift-Option-2 -> "@").
    // Other platforms keep Alt shortcuts unless they produced non-ASCII text.
    let mut consumed = mods & GHOSTTY_MODS_SHIFT as GhosttyMods;
    let option_as_alt = {
        #[cfg(target_os = "macos")]
        { gpui_macos::option_as_alt().uses_alt(k.option_side) }
        #[cfg(not(target_os = "macos"))]
        { false }
    };
    if m.alt && !option_as_alt && text.as_deref().is_some_and(|t| !t.is_empty() && (cfg!(target_os = "macos") || !t.is_ascii())) {
        consumed |= GHOSTTY_MODS_ALT as GhosttyMods;
    }
    Some(vt.encode_key(key, mods, consumed, text.as_deref(), unshifted))
}

fn control_alias_bytes(vt: &mut vt::Vt, key: GhosttyKey, text: &str, shifted: bool, legacy: Vec<u8>) -> Vec<u8> {
    // libghostty has no mode-2 getter. Probe with printable base text so its
    // encoder can preserve the chord when modifyOtherKeys requests CSI 27.
    let mods = GHOSTTY_MODS_CTRL as GhosttyMods | if shifted { GHOSTTY_MODS_SHIFT as GhosttyMods } else { 0 };
    let encoded = vt.encode_key(key, mods, 0, Some(text), text.chars().next());
    if encoded.starts_with(b"\x1b[27;") { encoded } else { legacy }
}

fn map_key(key: &str) -> Option<(GhosttyKey, bool, Option<char>)> {
    let named = match key {
        "enter" => GHOSTTY_KEY_ENTER,
        "tab" => GHOSTTY_KEY_TAB,
        "escape" => GHOSTTY_KEY_ESCAPE,
        "backspace" => GHOSTTY_KEY_BACKSPACE,
        "space" => return Some((GHOSTTY_KEY_SPACE, false, Some(' '))),
        "delete" => GHOSTTY_KEY_DELETE,
        "insert" => GHOSTTY_KEY_INSERT,
        "home" => GHOSTTY_KEY_HOME,
        "end" => GHOSTTY_KEY_END,
        "pageup" => GHOSTTY_KEY_PAGE_UP,
        "pagedown" => GHOSTTY_KEY_PAGE_DOWN,
        "up" => GHOSTTY_KEY_ARROW_UP,
        "down" => GHOSTTY_KEY_ARROW_DOWN,
        "left" => GHOSTTY_KEY_ARROW_LEFT,
        "right" => GHOSTTY_KEY_ARROW_RIGHT,
        _ => GHOSTTY_KEY_UNIDENTIFIED,
    };
    if named != GHOSTTY_KEY_UNIDENTIFIED {
        return Some((named, false, None));
    }
    // C enums are int on Linux and unsigned on macOS, so offsets take the constants' own type.
    if let Some(n) = key.strip_prefix('f').and_then(|n| n.parse::<u8>().ok())
        && (1..=12).contains(&n)
    {
        return Some((GHOSTTY_KEY_F1 + (n - 1) as GhosttyKey, false, None));
    }
    let mut chars = key.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else { return None };
    const SHIFTED: &[(char, char)] = &[
        ('~', '`'), ('!', '1'), ('@', '2'), ('#', '3'), ('$', '4'), ('%', '5'), ('^', '6'), ('&', '7'),
        ('*', '8'), ('(', '9'), (')', '0'), ('_', '-'), ('+', '='), ('{', '['), ('}', ']'), ('|', '\\'),
        (':', ';'), ('"', '\''), ('<', ','), ('>', '.'), ('?', '/'),
    ];
    let (base, shifted) = match SHIFTED.iter().find(|(s, _)| *s == c) {
        Some((_, b)) => (*b, true),
        None => (c, false),
    };
    let code = match base {
        'a'..='z' => GHOSTTY_KEY_A + (base as u8 - b'a') as GhosttyKey,
        '0'..='9' => GHOSTTY_KEY_DIGIT_0 + (base as u8 - b'0') as GhosttyKey,
        '`' => GHOSTTY_KEY_BACKQUOTE,
        '\\' => GHOSTTY_KEY_BACKSLASH,
        '[' => GHOSTTY_KEY_BRACKET_LEFT,
        ']' => GHOSTTY_KEY_BRACKET_RIGHT,
        ',' => GHOSTTY_KEY_COMMA,
        '=' => GHOSTTY_KEY_EQUAL,
        '-' => GHOSTTY_KEY_MINUS,
        '.' => GHOSTTY_KEY_PERIOD,
        '\'' => GHOSTTY_KEY_QUOTE,
        ';' => GHOSTTY_KEY_SEMICOLON,
        '/' => GHOSTTY_KEY_SLASH,
        _ => GHOSTTY_KEY_UNIDENTIFIED,
    };
    Some((code, shifted, Some(base)))
}

fn ligature_features(enabled: bool) -> FontFeatures {
    static FEATURES: std::sync::LazyLock<[FontFeatures; 2]> = std::sync::LazyLock::new(|| {
        [0, 1].map(|value| FontFeatures(Arc::new(vec![("calt".into(), value), ("liga".into(), value), ("dlig".into(), 0)])))
    });
    FEATURES[usize::from(enabled)].clone()
}

fn cell_font(bold: bool, italic: bool) -> Font {
    let mut f = font(crate::settings::terminal_font());
    f.features = ligature_features(false);
    let weight = crate::settings::get().font_weight.clamp(100, 900);
    f.weight = if bold { FontWeight::BOLD } else { FontWeight(weight as f32) };
    f.style = if italic { FontStyle::Italic } else { FontStyle::Normal };
    f
}

// JetBrains Mono joins punctuation, so ordinary ASCII output can keep using
// the ligatures-off font even when the setting is enabled.
fn needs_ligatures(text: &str, font: &Font) -> bool {
    font.family.as_ref() != crate::settings::DEFAULT_FONT
        || !text.is_ascii()
        || text.as_bytes().windows(2).any(|pair| pair[0].is_ascii_punctuation() && pair[1].is_ascii_punctuation())
}

#[derive(Clone, Copy, Debug)]
struct CellMetrics {
    size: Size<Pixels>,
    /// How far every glyph moves right in its cell, as Ghostty's `renderGlyph` (`glyph_x`).
    glyph_x: Pixels,
    baseline: Pixels,
    underline: Pixels,
    underline_thickness: Pixels,
    strike: Pixels,
    strike_thickness: Pixels,
}

fn cell_metrics(window: &Window) -> CellMetrics {
    let ts = window.text_system();
    let id = ts.resolve_font(&cell_font(false, false));
    let (fs, scale) = (crate::settings::font_size(), window.scale_factor());
    // Asked for on every frame; changes with the font, size, scale, line height or cell width.
    thread_local!(static LAST: Cell<Option<((FontId, u32, u32, u32, u32), CellMetrics)>> = const { Cell::new(None) });
    let s = crate::settings::get();
    let (line_height, cell_width) = (s.terminal_line_height, s.terminal_cell_width);
    let key = (id, fs.to_bits(), scale.to_bits(), line_height.to_bits(), cell_width.to_bits());
    if let Some((_, metrics)) = LAST.get().filter(|(last, _)| *last == key) {
        return metrics;
    }
    let metrics = font_cell_metrics(ts, id, fs, scale, line_height, cell_width);
    LAST.set(Some((key, metrics)));
    metrics
}

fn font_cell_metrics(ts: &WindowTextSystem, id: FontId, fs: f32, scale: f32, line_height: f32, cell_width: f32) -> CellMetrics {
    // FreeType uses integer y_ppem for metrics; CoreText uses the requested size.
    let em = if cfg!(target_os = "macos") { fs * scale } else { (fs * scale).round().max(1.0) };
    let metrics = ts.font_metrics(id);
    let unit = em / metrics.units_per_em.max(1) as f32;
    let hhea_data = ts.font_table(id, *b"hhea");
    let os2_data = ts.font_table(id, *b"OS/2");
    let post_data = ts.font_table(id, *b"post");
    let hhea = hhea_data.as_deref().and_then(ttf_parser::hhea::Table::parse);
    let os2 = os2_data.as_deref().and_then(ttf_parser::os2::Table::parse);
    let post = post_data.as_deref().and_then(ttf_parser::post::Table::parse);
    // Match Ghostty's hhea / USE_TYPO_METRICS / typo / Windows fallback order.
    let (ascent, descent, gap) = if let Some(hhea) = hhea {
        if let Some(os2) = os2 {
            if os2.use_typographic_metrics() {
                (os2.typographic_ascender() as f32, os2.typographic_descender() as f32, os2.typographic_line_gap() as f32)
            } else if hhea.ascender != 0 || hhea.descender != 0 {
                (hhea.ascender as f32, hhea.descender as f32, hhea.line_gap as f32)
            } else if os2.typographic_ascender() != 0 || os2.typographic_descender() != 0 {
                (os2.typographic_ascender() as f32, os2.typographic_descender() as f32, os2.typographic_line_gap() as f32)
            } else {
                (os2.windows_ascender() as f32, -(os2.windows_descender() as f32), 0.0)
            }
        } else {
            (hhea.ascender as f32, hhea.descender as f32, hhea.line_gap as f32)
        }
    } else {
        (metrics.ascent, metrics.descent, metrics.line_gap)
    };
    let face_height = (ascent - descent + gap) * unit;
    let height = face_height.round().max(1.0);
    let baseline_bottom = (gap * unit / 2.0 - descent * unit - (height - face_height) / 2.0).round();
    let baseline = height - baseline_bottom;
    let face_baseline = gap * unit / 2.0 - descent * unit;
    let (height, top) = adjusted_cell_height(height, face_height, baseline_bottom - face_baseline, line_height);
    let baseline = baseline + top;
    let measured_height = |ch| ts.typographic_bounds(id, px(metrics.units_per_em as f32), ch).ok()
        .map(|b| f32::from(b.size.height)).filter(|h| *h > 0.0);
    let ex = os2.and_then(|o| o.x_height()).filter(|h| *h > 0).map(|h| h as f32)
        .or_else(|| measured_height('x')).unwrap_or_else(|| {
            let cap = os2.and_then(|o| o.capital_height()).filter(|h| *h > 0).map(|h| h as f32)
                .or_else(|| measured_height('H')).unwrap_or(ascent * 0.75);
            cap * 0.75
        });
    let ul = post.map(|p| p.underline_metrics);
    let st = os2.map(|o| o.strikeout_metrics());
    let ul_thick = ul.filter(|m| m.thickness > 0).map_or(ex * unit * 0.15, |m| m.thickness as f32 * unit);
    let st_thick = st.filter(|m| m.thickness > 0).map_or(ul_thick, |m| m.thickness as f32 * unit);
    let ul_pos = ul.filter(|m| m.thickness != 0 || m.position != 0).map_or(-ul_thick, |m| m.position as f32 * unit);
    let st_pos = st.filter(|m| m.thickness != 0 || m.position != 0).map_or((ex * unit + st_thick) * 0.5, |m| m.position as f32 * unit);
    let face_width = (' '..='~').filter_map(|c| ts.advance(id, px(fs * scale), c).ok())
        .map(|s| f32::from(s.width)).reduce(f32::max).unwrap_or(fs * scale * 0.6);
    // As Ghostty's adjust-cell-width: scale the rounded cell; glyph_x re-centres the face in it.
    let width = adjusted_cell_width(face_width.round().max(1.0), cell_width);
    CellMetrics {
        size: size(px(width / scale), px(height / scale)),
        glyph_x: px(glyph_x(width, face_width) / scale),
        baseline: px(baseline / scale),
        underline: px((baseline - ul_pos).round() / scale),
        underline_thickness: px(ul_thick.ceil().max(1.0) / scale),
        strike: px((baseline - st_pos).round() / scale),
        strike_thickness: px(st_thick.ceil().max(1.0) / scale),
    }
}

// Ghostty's `Metrics` modifier: the cell times (1 + percent / 100) in f64, rounded. Taking the
// product in f32 from the multiplier can land on the other side of a half pixel (30 px at 1.05).
fn adjusted_cell_width(width: f32, multiplier: f32) -> f32 {
    let percent = ((f64::from(multiplier) - 1.0) * 100.0 * 1e4).round() / 1e4;
    ((f64::from(width) * (1.0 + percent / 100.0)).round() as f32).max(1.0)
}

// Ghostty puts an odd extra pixel on the edge that best centres the face.
fn adjusted_cell_height(height: f32, face_height: f32, face_y: f32, multiplier: f32) -> (f32, f32) {
    let adjusted = (height * multiplier).round().max(1.0);
    let half = (adjusted - height) / 2.0;
    let top = if face_y - (height - face_height) / 2.0 > 0.0 { half.ceil() } else { half.floor() };
    (adjusted, top)
}

pub fn cell_size(window: &Window) -> Size<Pixels> {
    cell_metrics(window).size
}

/// Ghostty's re-centring of glyphs in their cells (`renderGlyph`), in device
/// pixels: half the cell's difference from the face's unrounded advance.
/// CoreText (`font/face/coretext.zig`) keeps, when the cell is the narrower,
/// only the part under a whole pixel, so glyphs keep their subpixel phase
/// without sliding a pixel left; FreeType (`font/face/freetype.zig`) moves
/// only into a wider cell, by whole pixels, to keep hinting.
fn glyph_x(cell_width: f32, face_width: f32) -> f32 {
    let dx = (cell_width - face_width) / 2.0;
    if cfg!(target_os = "macos") {
        if dx < 0.0 { dx - dx.trunc() } else { dx }
    } else if dx > 0.0 {
        dx.round()
    } else {
        0.0
    }
}

/// Where a cell's shaped text paints from: its baseline on the cell's, moved across by `glyph_x`.
fn text_origin(line: &ShapedLine, origin: Point<Pixels>, metrics: CellMetrics) -> Point<Pixels> {
    let gpui_baseline = (metrics.size.height - line.ascent - line.descent) / 2.0 + line.ascent;
    origin + point(metrics.glyph_x, metrics.baseline - gpui_baseline)
}

fn paint_cell_text(line: &ShapedLine, origin: Point<Pixels>, metrics: CellMetrics, window: &mut Window, cx: &mut App) {
    let origin = text_origin(line, origin, metrics);
    let _ = line.paint(origin, metrics.size.height, TextAlign::Left, None, window, cx);
}

// Force-width shaping moves glyph origins, but does not resize fallback glyphs.
fn fallback_size(c: &vt::Cell, font: &Font, width: Pixels, ts: &gpui::WindowTextSystem) -> Option<Pixels> {
    if c.wide || c.text.is_ascii() {
        return None;
    }
    let first = c.text.chars().next()?;
    let size = px(crate::settings::font_size());
    let primary = ts.resolve_font(font);
    if ts.advance(primary, size, first).is_ok() {
        return None;
    }
    let run = TextRun { len: c.text.len(), font: font.clone(), ..Default::default() };
    let line = ts.shape_line(c.text.clone().into(), size, &[run], None);
    (line.width() > width).then(|| size * f32::from(width) / f32::from(line.width()))
}

/// One piece of a row's drawing, in cells from the row's start.
enum RowOp {
    Bg { from: usize, to: usize, color: Hsla },
    Sprite { x: usize, text: String, color: Hsla },
    Icon { x: usize, offset: Point<Pixels>, font: FontId, glyph: GlyphId, size: Pixels, color: Hsla },
    Text { x: usize, line: ShapedLine },
    /// An underline in its style and color, a strikethrough in the text's color.
    Decoration { from: usize, to: usize, underline: Option<(vt::Underline, Hsla)>, strike: Option<Hsla> },
}

/// A row's drawing: its backgrounds, merged into runs of one color, then its
/// text in runs of one style (wide characters, sprites and icons on their own),
/// then its underlines and strikethroughs.
fn row_ops(row: &[vt::Cell], cell: Size<Pixels>, cursor: Option<usize>, window: &mut Window) -> Vec<RowOp> {
    crate::bench::count(&crate::bench::ROWS);
    let ligatures = crate::settings::get().font_ligatures;
    let mut ops = Vec::new();
    let mut x = 0;
    while x < row.len() {
        let Some(bg) = row[x].bg else {
            x += 1;
            continue;
        };
        let start = x;
        while x < row.len() && row[x].bg == Some(bg) {
            x += 1;
        }
        ops.push(RowOp::Bg { from: start, to: x, color: hsla(bg) });
    }
    let mut x = 0;
    while x < row.len() {
        let start = x;
        let c = &row[start];
        // Consume the first cell before dispatch: every path must make progress,
        // even when a private-use character has no bundled icon glyph.
        x += 1;
        if c.text.is_empty() && c.underline.is_none() && !c.strike {
            continue;
        }
        let color = cell_color(c);
        if crate::sprites::is_sprite(&c.text) {
            ops.push(RowOp::Sprite { x: start, text: c.text.clone(), color });
            continue;
        }
        let mut text_font = cell_font(c.bold, c.italic);
        if let Some(icon) = fallback_icon(&c.text, &text_font, window.text_system()) {
            // Fit missing icons from the bundled font by their outlines:
            // centered, as tall as fits, two cells wide when a space
            // follows (prompts, statuslines, file trees).
            let room = if row.get(x).is_none_or(|d| d.text.trim().is_empty()) { 2.0 } else { 1.0 };
            let (w, h) = (f32::from(cell.width) * room, f32::from(cell.height));
            let (gw, gh) = ((icon.x1 - icon.x0).max(1.0), (icon.y1 - icon.y0).max(1.0));
            // Up to ~1.5 cells of two, so it doesn't touch the next word.
            let scale = (w * if room > 1.0 { 0.75 } else { 0.9 } / gw).min(h * 0.72 / gh);
            // Centered in its own cell; a wider icon starts at the cell
            // and spills into the space after it.
            let cw = f32::from(cell.width);
            let left = ((cw - gw * scale) / 2.0).max(0.0) - icon.x0 * scale;
            let baseline = (h - gh * scale) / 2.0 + icon.y1 * scale;
            let font = window.text_system().resolve_font(&font(crate::settings::SYMBOLS_FONT));
            let size = px(scale * icon.units_per_em);
            ops.push(RowOp::Icon { x: start, offset: point(px(left), px(baseline)), font, glyph: GlyphId(icon.id), size, color });
            continue;
        }
        let fitted_size = fallback_size(c, &text_font, cell.width, window.text_system());
        let mut text = if c.text.is_empty() { " ".to_owned() } else { c.text.clone() };
        let mut positions = vec![(0, px(0.0))];
        // Shape the cursor cell separately, so its plain glyph doesn't cover a ligature.
        if !c.wide && fitted_size.is_none() && cursor != Some(start) {
            while x < row.len() {
                let d = &row[x];
                let same = !d.wide
                    && (!ligatures || d.bg == c.bg)
                    && d.fg == c.fg
                    && d.bold == c.bold
                    && d.italic == c.italic
                    && d.faint == c.faint
                    && d.underline == c.underline
                    && d.strike == c.strike;
                // Bundled fallback icons are fitted separately from the text run.
                if !same || cursor == Some(x) || (d.text.is_empty() && d.underline.is_none() && !d.strike) || crate::sprites::is_sprite(&d.text) || fallback_icon(&d.text, &text_font, window.text_system()).is_some() || fallback_size(d, &text_font, cell.width, window.text_system()).is_some() {
                    break;
                }
                positions.push((text.len(), cell.width * (x - start) as f32));
                text.push_str(if d.text.is_empty() { " " } else { &d.text });
                x += 1;
            }
        }
        if text.trim_matches(' ').is_empty() && c.underline.is_none() && !c.strike {
            continue;
        }
        if ligatures && needs_ligatures(&text, &text_font) {
            text_font.features = ligature_features(true);
        }
        let run = TextRun {
            len: text.len(),
            font: text_font,
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let width = cell.width * if c.wide { 2.0 } else { (x - start) as f32 };
        let line = window.text_system().shape_line(text.into(), fitted_size.unwrap_or_else(|| px(crate::settings::font_size())), &[run], None)
            .with_cell_positions(&positions, width);
        ops.push(RowOp::Text { x: start, line });
    }
    row_decorations(row, &mut ops);
    ops
}

/// A cell's text color, dimmed when faint.
fn cell_color(c: &vt::Cell) -> Hsla {
    hsla(c.fg).opacity(if c.faint { 0.6 } else { 1.0 })
}

/// A row's underlines and strikethroughs, from its cells alone, so sprites,
/// icons and blanks get them too. Neighbours that look the same share one.
fn row_decorations(row: &[vt::Cell], ops: &mut Vec<RowOp>) {
    let mut x = 0;
    while x < row.len() {
        let c = &row[x];
        let start = x;
        x += if c.wide { 2 } else { 1 };
        if c.underline.is_none() && !c.strike {
            continue;
        }
        let color = cell_color(c);
        let underline = c.underline.map(|s| (s, c.underline_color.map_or(color, |u| hsla(u).opacity(color.a))));
        let strike = c.strike.then_some(color);
        match ops.last_mut() {
            Some(RowOp::Decoration { to, underline: u, strike: s, .. }) if *to == start && *u == underline && *s == strike => *to = x,
            _ => ops.push(RowOp::Decoration { from: start, to: x, underline, strike }),
        }
    }
}

/// The find field's editor colors, following the theme at open time.
fn find_input_style() -> InputEditorStyle {
    InputEditorStyle {
        foreground: crate::settings::color("foreground"),
        muted_foreground: crate::settings::color("muted"),
        background: crate::settings::color("input"),
        border: crate::settings::color("border"),
        selection: crate::settings::color("selection"),
        caret: crate::settings::color("foreground"),
        ..InputEditorStyle::default()
    }
}

fn paint_grid(bounds: Bounds<Pixels>, grid: &Rc<RefCell<Grid>>, focused: bool, window: &mut Window, cx: &mut App) {
    crate::bench::count(&crate::bench::GRID);
    let mut g = grid.borrow_mut();
    let cell = g.cell;
    let metrics = cell_metrics(window);
    let (pad_x, pad_y) = pad();
    // Rows rarely fill the height exactly: split what's left above and below.
    let spare = (f32::from(bounds.size.height) - 2.0 * pad_y - f32::from(cell.height) * g.vt.rows as f32).max(0.0);
    let scale = window.scale_factor();
    let snap = |v: Pixels| px((f32::from(v) * scale).round() / scale);
    let origin = bounds.origin + point(px(pad_x), px(pad_y + (spare / 2.0).floor()));
    let origin = point(snap(origin.x), snap(origin.y));
    g.origin = origin;
    let frame = g.vt.frame();
    let placements = g.vt.placements();
    let mut images: Vec<(bool, Bounds<Pixels>, Arc<RenderImage>)> = Vec::new();
    let mut keep = std::collections::HashSet::new();
    for p in &placements {
        let key = (p.image, p.generation, p.source);
        keep.insert(key);
        if !g.images.contains_key(&key)
            && let Some((w, h, bgra)) = g.vt.image_bgra(p.image, p.source)
            && let Some(buf) = image::RgbaImage::from_raw(w, h, bgra)
        {
            g.images.insert(key, Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(buf)])));
        }
        if let Some(img) = g.images.get(&key) {
            let [x, y, mut width, mut height] = p.pixel_geometry(f32::from(cell.width), f32::from(cell.height));
            if p.cell_pixels.is_none() {
                // Direct placements are in device pixels.
                width /= scale;
                height /= scale;
            }
            let pos = origin + point(px(x), px(y));
            // Placeholder runs contain no text and must cover explicit cell backgrounds.
            let below = p.z < 0 && p.cell_pixels.is_none();
            images.push((below, Bounds::new(pos, size(px(width), px(height))), img.clone()));
        }
    }
    g.images.retain(|k, _| keep.contains(k));
    let link_spans = g.link_spans.clone();
    let marked = g.marked.clone();
    drop(g);

    window.paint_quad(fill(bounds, hsla(frame.bg).opacity(crate::settings::get().terminal_opacity)));
    let at = |x: usize, y: usize| origin + point(cell.width * x as f32, cell.height * y as f32);
    // Edges round to device pixels, so cells side by side meet without seams.
    let snap_box = |b: Bounds<Pixels>| {
        let (x0, x1) = (snap(b.origin.x), snap(b.origin.x + b.size.width));
        let (y0, y1) = (snap(b.origin.y), snap(b.origin.y + b.size.height));
        Bounds::new(point(x0, y0), size(x1 - x0, y1 - y0))
    };
    let paint_images = |below: bool, window: &mut Window| {
        for (image_below, b, img) in &images {
            if *image_below == below {
                let _ = window.paint_image(*b, *b, Corners::default(), img.clone(), 0, false);
            }
        }
    };
    paint_images(true, window);

    // Each row's drawing is built once and painted again while the row is the
    // same (`Vt::frame` keeps a row libghostty didn't change): an agent's spinner
    // line rebuilds one row, not the whole screen.
    let ligatures = crate::settings::get().font_ligatures;
    let cursor = frame.cursor_cell.filter(|_| ligatures);
    // The whole cell font, so a weight change redraws kept rows too.
    let with = (crate::settings::font_size().to_bits(), cell, cell_font(false, false), crate::settings::fonts_added(), ligatures);
    // When every row changed (output scrolling), nothing is kept: the next
    // frame would find none of them the same.
    let mut painted = std::mem::take(&mut grid.borrow_mut().painted);
    if grid.borrow().painted_with != with || frame.all {
        painted.clear();
    }
    painted.truncate(frame.rows.len());
    for (y, row) in frame.rows.iter().enumerate() {
        let column = cursor.filter(|(_, cy)| *cy as usize == y).map(|(x, _)| x as usize);
        if frame.all || painted.get(y).is_none_or(|(r, _, c)| !Rc::ptr_eq(r, row) || *c != column) {
            let ops = row_ops(row, cell, column, window);
            if y < painted.len() {
                painted[y] = (row.clone(), ops, column);
            } else {
                painted.push((row.clone(), ops, column));
            }
        }
    }

    // Paint all program backgrounds first, then search overlays, then text.
    // This lets a match highlight sit below glyphs without contaminating
    // cached RowOps or the terminal's own selection/copy state.
    for (y, (_, ops, _)) in painted.iter().enumerate() {
        for op in ops {
            match op {
                RowOp::Bg { from, to, color } => {
                    let b = Bounds::new(at(*from, y), size(cell.width * (to - from) as f32, cell.height));
                    window.paint_quad(fill(snap_box(b), *color));
                }
                _ => {}
            }
        }
    }
    for selected in [false, true] {
        for span in frame.search_spans.iter().filter(|span| span.selected == selected) {
            let y = span.row as usize;
            let Some(row) = frame.rows.get(y) else { continue };
            let start = span.start as usize;
            let end = (span.end as usize + 1).min(row.len());
            if start >= end {
                continue;
            }
            let opacity = if selected { 0.55 } else { 0.28 };
            let highlight = crate::settings::color("accent").opacity(opacity);
            let b = Bounds::new(at(start, y), size(cell.width * (end - start) as f32, cell.height));
            window.paint_quad(fill(snap_box(b), highlight));
        }
    }
    for (y, (_, ops, _)) in painted.iter().enumerate() {
        for op in ops {
            match op {
                RowOp::Bg { .. } => {}
                RowOp::Sprite { x, text, color } => crate::sprites::paint(text, Bounds::new(at(*x, y), cell), *color, window),
                RowOp::Icon { x, offset, font, glyph, size, color } => {
                    let _ = window.paint_glyph(at(*x, y) + *offset, *font, *glyph, *size, *color);
                }
                RowOp::Text { x, line } => {
                    paint_cell_text(line, at(*x, y), metrics, window, cx);
                }
                RowOp::Decoration { from, to, underline, strike } => {
                    let width = cell.width * (to - from) as f32;
                    let line = |offset: Pixels, thickness: Pixels| snap_box(Bounds::new(at(*from, y) + point(px(0.0), offset), size(width, thickness)));
                    let (top, thick) = (metrics.underline, metrics.underline_thickness);
                    match *underline {
                        None => {}
                        Some((vt::Underline::Single, color)) => window.paint_quad(fill(line(top, thick), color)),
                        Some((vt::Underline::Double, color)) => {
                            // A line above and a line below where the single one is, as Ghostty draws it.
                            let top = top.min(cell.height - thick * 2.0);
                            window.paint_quad(fill(line(top - thick, thick), color));
                            window.paint_quad(fill(line(top + thick, thick), color));
                        }
                        Some((vt::Underline::Curly, color)) => {
                            // GPUI's wave is three thicknesses tall, around its middle.
                            let offset = (top - thick).min(cell.height - thick * 3.0);
                            let style = UnderlineStyle { thickness: thick, color: Some(color), wavy: true };
                            window.paint_underline(at(*from, y) + point(px(0.0), offset), width, &style);
                        }
                        Some((vt::Underline::Dotted, color)) => {
                            // Dots wider than the line and about a dot apart, at least one a cell, as Ghostty draws them.
                            let (width, radius) = (f32::from(cell.width) * scale, std::f32::consts::FRAC_1_SQRT_2 * f32::from(thick) * scale);
                            let count = (width / (4.0 * radius)).ceil().min((width / (3.0 * radius)).floor()).min((width / (2.0 * radius + 1.0)).floor()).max(1.0);
                            let (step, dot) = (cell.width / count, px(2.0 * radius / scale));
                            let offset = (top + (thick - dot) / 2.0).min(cell.height - dot);
                            for i in 0..(to - from) * count as usize {
                                let b = Bounds::new(at(*from, y) + point(step * (i as f32 + 0.5) - dot / 2.0, offset), size(dot, dot));
                                window.paint_quad(fill(b, color).corner_radii(dot / 2.0));
                            }
                        }
                        Some((vt::Underline::Dashed, color)) => {
                            // In every cell a dash, a gap, and the start of the next dash, as Ghostty draws it.
                            let width = f32::from(cell.width) * scale;
                            let dash = (width / 3.0).floor() + 1.0;
                            for x in *from..*to {
                                for (start, len) in [(0.0, dash.min(width)), (2.0 * dash, (width - 2.0 * dash).max(0.0))] {
                                    let b = Bounds::new(at(x, y) + point(px(start / scale), top), size(px(len / scale), thick));
                                    window.paint_quad(fill(snap_box(b), color));
                                }
                            }
                        }
                    }
                    if let Some(color) = *strike {
                        window.paint_quad(fill(line(metrics.strike, metrics.strike_thickness), color));
                    }
                }
            }
        }
    }
    {
        let mut g = grid.borrow_mut();
        (g.painted, g.painted_with) = (painted, with);
    }

    for &(y, x0, x1) in &link_spans {
        let Some(row) = frame.rows.get(y as usize) else { continue };
        let color = row.get(x0 as usize).map_or(hsla(vt::theme().fg), |c| hsla(c.fg));
        let (x0, x1) = (x0 as usize, (x1 as usize + 1).min(row.len()));
        let line = Bounds::new(at(x0, y as usize) + point(px(0.0), metrics.underline), size(cell.width * (x1 - x0) as f32, metrics.underline_thickness));
        window.paint_quad(fill(snap_box(line), color));
    }

    paint_images(false, window);

    if let Some(cur) = frame.cursor {
        let (x, y) = (cur.x as usize, cur.y as usize);
        let wide = frame.rows.get(y).and_then(|r| r.get(x)).is_some_and(|c| c.wide);
        let w = cell.width * if wide { 2.0 } else { 1.0 };
        let color = hsla(cur.color);
        let p = at(x, y);
        match (focused, cur.shape) {
            (true, CursorShape::Block) => {
                window.paint_quad(fill(snap_box(Bounds::new(p, size(w, cell.height))), color));
                if let Some(c) = frame.rows.get(y).and_then(|r| r.get(x)).filter(|c| !c.text.is_empty()) {
                    let run = TextRun {
                        len: c.text.len(),
                        font: cell_font(c.bold, c.italic),
                        color: hsla(frame.bg),
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    };
                    let line = window.text_system().shape_line(c.text.clone().into(), px(crate::settings::font_size()), &[run], None);
                    paint_cell_text(&line, p, metrics, window, cx);
                }
            }
            (true, CursorShape::Bar) => window.paint_quad(fill(snap_box(Bounds::new(p, size(px(2.0), cell.height))), color)),
            (true, CursorShape::Underline) => window.paint_quad(fill(
                snap_box(Bounds::new(p + point(px(0.0), cell.height - px(2.0)), size(w, px(2.0)))),
                color,
            )),
            _ => window.paint_quad(outline(snap_box(Bounds::new(p, size(w, cell.height))), color, BorderStyle::Solid)),
        }
    }
    // What an input method is composing, at the cursor (hidden or not):
    // underlined, on the terminal's background so the cells under it don't
    // show through.
    if let (Some((cx_, cy_)), Some(text)) = (frame.cursor_cell, marked.filter(|t| !t.is_empty())) {
        let p = at(cx_ as usize, cy_ as usize);
        let fg = hsla(vt::theme().fg);
        let run = TextRun {
            len: text.len(),
            font: cell_font(false, false),
            color: fg,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let line = window.text_system().shape_line(text.into(), px(crate::settings::font_size()), &[run], None);
        window.paint_quad(fill(snap_box(Bounds::new(p, size(line.width, cell.height))), hsla(frame.bg)));
        paint_cell_text(&line, p, metrics, window, cx);
        let underline = Bounds::new(p + point(px(0.0), metrics.underline), size(line.width, metrics.underline_thickness));
        window.paint_quad(fill(snap_box(underline), fg));
    }
    {
        let mut g = grid.borrow_mut();
        g.cursor_at = frame.cursor_cell.map(|(x, y)| Bounds::new(at(x as usize, y as usize), cell));
        // As in Herdr, hide on the alternate screen and below five columns.
        g.thumb = None;
        g.track = None;
        let sb = g.vt.scrollbar();
        let track = f32::from(cell.height) * f32::from(g.vt.rows);
        if crate::settings::get().pane_scrollbars
            && !g.vt.alt_screen()
            && g.vt.cols > 4
            && let Some((top, len)) = scrollbar_thumb(sb.total, sb.offset, sb.len, track, f32::from(cell.height))
        {
            let lane = scrollbar_lane(bounds, origin, cell.width * f32::from(g.vt.cols), px(track));
            let w = px(4.0);
            let x = lane.origin.x + lane.size.width - w - px(2.0);
            let color = crate::settings::color("muted");
            let rail = Bounds::new(point(x + w / 2.0 - px(0.5), origin.y), size(px(1.0), px(track)));
            window.paint_quad(fill(snap_box(rail), color.opacity(0.35)));
            let thumb = Bounds::new(point(x, origin.y + px(top)), size(w, px(len)));
            window.paint_quad(fill(snap_box(thumb), color).corner_radii(w / 2.0));
            g.thumb = Some(Bounds::new(point(lane.origin.x, thumb.origin.y), size(lane.size.width, thumb.size.height)));
            g.track = Some(lane);
        }
    }
}

/// OS text input (AltGr, Pinyin, Japanese, Korean, dead keys) types here.
/// Native composition previews reach this handler; other keys go through
/// `key_bytes`. The program gets the finished text as it is.
impl EntityInputHandler for TerminalView {
    fn text_for_range(&mut self, range: std::ops::Range<usize>, adjusted: &mut Option<std::ops::Range<usize>>, _w: &mut Window, _cx: &mut Context<Self>) -> Option<String> {
        let marked = self.grid.borrow().marked.clone().unwrap_or_default();
        let units: Vec<u16> = marked.encode_utf16().collect();
        let range = range.start.min(units.len())..range.end.min(units.len());
        *adjusted = Some(range.clone());
        Some(String::from_utf16_lossy(&units[range]))
    }

    fn selected_text_range(&mut self, _ignore_disabled: bool, _w: &mut Window, _cx: &mut Context<Self>) -> Option<UTF16Selection> {
        let len = self.grid.borrow().marked.as_ref().map_or(0, |m| m.encode_utf16().count());
        Some(UTF16Selection { range: len..len, reversed: false })
    }

    fn marked_text_range(&self, _w: &mut Window, _cx: &mut Context<Self>) -> Option<std::ops::Range<usize>> {
        self.grid.borrow().marked.as_ref().filter(|m| !m.is_empty()).map(|m| 0..m.encode_utf16().count())
    }

    fn unmark_text(&mut self, _w: &mut Window, cx: &mut Context<Self>) {
        self.grid.borrow_mut().marked = None;
        cx.notify();
    }

    fn replace_text_in_range(&mut self, _range: Option<std::ops::Range<usize>>, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.grid.borrow_mut().marked = None;
        // While the find field holds the keys, the input-method path must not
        // be a way around that (see `key_down`); with the terminal focused,
        // input methods type into the program as usual.
        if !self.find_field_focused(window, cx) {
            self.send(text.as_bytes());
        }
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<std::ops::Range<usize>>,
        text: &str,
        _selected: Option<std::ops::Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Composition previews stay out of the terminal too while the find
        // field holds the keys; with the terminal focused they show as usual.
        if self.find_field_focused(window, cx) {
            return;
        }
        self.grid.borrow_mut().marked = (!text.is_empty()).then(|| text.to_owned());
        cx.notify();
    }

    fn bounds_for_range(&mut self, range: std::ops::Range<usize>, _element: Bounds<Pixels>, _w: &mut Window, _cx: &mut Context<Self>) -> Option<Bounds<Pixels>> {
        let g = self.grid.borrow();
        let at = g.cursor_at?;
        Some(Bounds::new(at.origin + point(g.cell.width * range.start as f32, px(0.0)), at.size))
    }

    fn character_index_for_point(&mut self, _point: Point<Pixels>, _w: &mut Window, _cx: &mut Context<Self>) -> Option<usize> {
        None
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.publish_scroll_if_changed(cx);
        crate::bench::count(&crate::bench::TERMINAL);
        let terminal_focused = self.focus.is_focused(window);
        if terminal_focused {
            self.search_keys = false;
        }
        let search_focused = self.find_field_focused(window, cx);
        // The find input is part of this terminal's interaction, not a loss of
        // attention to the terminal itself. Keep core focus/cursor state, while
        // only installing the terminal's IME handler when its own focus handle
        // owns keyboard input.
        let focused = terminal_focused || search_focused;
        if focused != self.was_focused {
            self.was_focused = focused;
            let g = self.grid.borrow();
            if let Some(ch) = g.channel {
                g.core.focus(ch, focused);
            }
        }
        let cell = cell_size(window);
        let hover = self.grid.borrow().hover;
        let found = hover.filter(|_| link_mods(&window.modifiers())).and_then(|pos| self.link_at(pos));
        let (link, spans) = found.unzip();
        self.link = link;
        self.grid.borrow_mut().link_spans = spans.unwrap_or_default();
        let grid = self.grid.clone();
        let grid_paint = self.grid.clone();
        let selection_view = cx.entity().downgrade();
        let (input_focus, input_view) = (self.focus.clone(), cx.entity());
        let ended = self.ended.clone().or_else(|| self.lost.then(|| "reconnecting to core…".to_owned()));
        let search_input = self.search_open.then(|| self.search_input.clone()).flatten();
        let search_progress = self.grid.borrow().vt.search_progress();
        let search_label = if self.search_query.is_empty() {
            String::new()
        } else if !search_progress.complete {
            if search_progress.total == 0 { "Searching…".to_owned() } else { format!("{}+", search_progress.total) }
        } else if search_progress.total == 0 {
            "No matches".to_owned()
        } else if let Some(selected) = search_progress.selected {
            format!("{} / {}", selected + 1, search_progress.total)
        } else {
            format!("{} matches", search_progress.total)
        };
        let address = self.address.clone();

        div()
            .id(SharedString::from(format!("term-{}", self.address)))
            .track_focus(&self.focus)
            .key_context("Terminal")
            .size_full()
            .relative()
            // Below 1, the layout's background shows through (cells a program colors stay solid).
            .bg(hsla(vt::theme().bg).opacity(crate::settings::get().terminal_opacity))
            .on_key_down(cx.listener(Self::key_down))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| this.dropped(paths.paths().to_vec(), cx)))
            .on_scroll_wheel(cx.listener(Self::scroll))
            .when(self.link.is_some(), |d| d.cursor_pointer())
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !hovered {
                    this.grid.borrow_mut().hover = None;
                    if this.link.is_some() {
                        cx.notify();
                    }
                }
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                this.focus.focus(window, cx);
                let g = this.grid.borrow();
                if let Some(ch) = g.channel {
                    g.core.resize(ch, g.vt.cols, g.vt.rows);
                }
                drop(g);
                if link_mods(&ev.modifiers)
                    && let Some((target, _)) = this.link_at(ev.position)
                {
                    match target {
                        LinkTarget::External(target) => links::open(&target),
                        LinkTarget::Terminal(address) => cx.emit(TerminalEvent::Peek(address)),
                    }
                    return;
                }
                let mut g = this.grid.borrow_mut();
                if let (Some(track), Some(thumb)) = (g.track, g.thumb)
                    && track.contains(&ev.position)
                {
                    let grab = scrollbar_grab_at(f32::from(thumb.origin.y), f32::from(thumb.size.height), f32::from(ev.position.y));
                    this.scrollbar_grab = Some(grab);
                    if !thumb.contains(&ev.position) {
                        let sb = g.vt.scrollbar();
                        let top = f32::from(ev.position.y - track.origin.y) - grab;
                        let min = f32::from(g.cell.height);
                        g.vt.scroll_to_row(scrollbar_row_at(sb.total, sb.len, f32::from(track.size.height), min, top));
                    }
                    drop(g);
                    this.publish_scroll_if_changed(cx);
                    cx.notify();
                    cx.stop_propagation();
                    return;
                }
                drop(g);
                if this.mouse(GHOSTTY_MOUSE_ACTION_PRESS, Some(ev.button), ev.position, ev.modifiers) {
                    // The program took the click: Herdr's old selection ends, so a copy can't repeat it.
                    this.clear_selection(cx);
                } else {
                    this.start_selection(ev.position, cx);
                }
            }))
            // Right-click is Herdr's menu, even when the program tracks the mouse
            // (agent TUIs do, and then a pane had no menu at all), unless the pane
            // sends right-clicks to its program.
            .on_mouse_down(MouseButton::Right, cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                if this.right_click_to_program && this.takes_mouse(ev.modifiers) {
                    this.mouse(GHOSTTY_MOUSE_ACTION_PRESS, Some(ev.button), ev.position, ev.modifiers);
                } else {
                    cx.emit(TerminalEvent::ContextMenu(ev.position));
                }
            }))
            .on_mouse_down(MouseButton::Middle, cx.listener(|this, ev: &MouseDownEvent, _, _cx| {
                if !this.mouse(GHOSTTY_MOUSE_ACTION_PRESS, Some(ev.button), ev.position, ev.modifiers) {
                    // macOS has no PRIMARY selection, so an unclaimed middle-click does nothing.
                    #[cfg(target_os = "linux")]
                    if let Some(text) = _cx.read_from_primary().and_then(|item| item.text()).filter(|text| !text.is_empty()) {
                        let bytes = this.grid.borrow().vt.paste_bytes(&text);
                        this.send(&bytes);
                    }
                    #[cfg(target_os = "windows")]
                    this.paste(_cx);
                }
            }))
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                this.grid.borrow_mut().hover = Some(ev.position);
                if link_mods(&ev.modifiers) || this.link.is_some() {
                    cx.notify();
                }
                if !this.selecting && ev.pressed_button.is_none() {
                    this.mouse(GHOSTTY_MOUSE_ACTION_MOTION, None, ev.position, ev.modifiers);
                }
            }))
            .child(
                canvas(
                    move |bounds, window, _cx| {
                        let (cols, rows) = grid_size(bounds.size, cell);
                        grid.borrow_mut().fit(cols, rows, cell, window.scale_factor());
                    },
                    move |bounds, _, window, cx| {
                        paint_grid(bounds, &grid_paint, focused, window, cx);
                        let view = selection_view.clone();
                        window.on_mouse_event(move |ev: &MouseMoveEvent, phase, _, cx| {
                            if phase == DispatchPhase::Capture {
                                let _ = view.update(cx, |this, cx| {
                                    if this.scrollbar_grab.is_some() {
                                        if ev.pressed_button != Some(MouseButton::Left) {
                                            this.scrollbar_grab = None;
                                            return;
                                        }
                                        let mut g = this.grid.borrow_mut();
                                        let sb = g.vt.scrollbar();
                                        let (track, min) = (f32::from(g.cell.height) * f32::from(g.vt.rows), f32::from(g.cell.height));
                                        let top = f32::from(ev.position.y - g.origin.y) - this.scrollbar_grab.unwrap_or(0.0);
                                        g.vt.scroll_to_row(scrollbar_row_at(sb.total, sb.len, track, min, top));
                                        drop(g);
                                        this.publish_scroll_if_changed(cx);
                                        cx.notify();
                                        return;
                                    }
                                    if !this.selecting && ev.pressed_button.is_some() && !this.grid.borrow().buttons_down.is_empty() {
                                        this.mouse(GHOSTTY_MOUSE_ACTION_MOTION, ev.pressed_button, ev.position, ev.modifiers);
                                    }
                                    if this.selecting {
                                        this.selection_pointer = ev.position;
                                        if ev.pressed_button == Some(MouseButton::Left) {
                                            this.select(Select::Drag, ev.position, cx);
                                        } else {
                                            this.selection_scroll = None;
                                            this.finish_selection(Some(ev.position), cx);
                                        }
                                    }
                                });
                            }
                        });
                        window.on_mouse_event(move |ev: &MouseUpEvent, phase, _, cx| {
                            if phase == DispatchPhase::Capture {
                                let _ = selection_view.update(cx, |this, cx| {
                                    if this.scrollbar_grab.is_some() {
                                        if ev.button == MouseButton::Left {
                                            this.scrollbar_grab = None;
                                            cx.stop_propagation();
                                        }
                                        return;
                                    }
                                    if (!this.selecting || ev.button != MouseButton::Left) && this.grid.borrow().buttons_down.contains(&ev.button) {
                                        this.mouse(GHOSTTY_MOUSE_ACTION_RELEASE, Some(ev.button), ev.position, ev.modifiers);
                                    }
                                    if this.selecting && ev.button == MouseButton::Left {
                                        this.selection_scroll = None;
                                        this.finish_selection(Some(ev.position), cx);
                                        cx.stop_propagation();
                                    }
                                });
                            }
                        });
                        // While focused, the OS input methods (Pinyin, Japanese,
                        // dead keys) type into this terminal.
                        if terminal_focused {
                            window.handle_input(&input_focus, ElementInputHandler::new(bounds, input_view), cx);
                        }
                    },
                )
                .size_full(),
            )
            .when_some(search_input, |d, input| {
                d.child(
                    div()
                        .flex()
                        .id(SharedString::from(format!("find-bar-{address}")))
                        .debug_selector(|| "find-bar".to_owned())
                        .absolute()
                        .top_2()
                        .right_2()
                        .w_full()
                        .max_w(px(460.0))
                        .h(px(38.0))
                        .gap_1()
                        .items_center()
                        .px_1()
                        .rounded_md()
                        .border_1()
                        .border_color(crate::settings::color("border"))
                        .bg(crate::settings::color("popover"))
                        .shadow_md()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(div().flex_1().min_w(px(48.0)).h_full().child(Input::new(&input)))
                        .child(div().w(px(58.0)).text_xs().text_color(crate::settings::color("muted")).child(search_label))
                        .child(
                            div()
                                .id(SharedString::from(format!("find-next-{address}")))
                                .debug_selector(|| "find-up".to_owned())
                                .w(px(26.0))
                                .h(px(26.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_sm()
                                .hover(|d| d.bg(crate::settings::color("hover")))
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.queue_search_navigation(false, cx);
                                    this.focus_search_input(window, cx);
                                }))
                                .child("↑"),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("find-prev-{address}")))
                                .debug_selector(|| "find-down".to_owned())
                                .w(px(26.0))
                                .h(px(26.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_sm()
                                .hover(|d| d.bg(crate::settings::color("hover")))
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.queue_search_navigation(true, cx);
                                    this.focus_search_input(window, cx);
                                }))
                                .child("↓"),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("find-close-{address}")))
                                .w(px(26.0))
                                .h(px(26.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_sm()
                                .hover(|d| d.bg(crate::settings::color("hover")))
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, window, cx| this.close_search(window, cx)))
                                .child("×"),
                        ),
                )
            })
            .when_some(ended, |d, why| {
                d.child(
                    div()
                        .absolute()
                        .bottom_2()
                        .right_3()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .bg(crate::settings::color("raised"))
                        .text_color(hsla(vt::theme().fg))
                        .text_xs()
                        .child(format!("[{why}]")),
                )
            })
            .when_some(self.sending.last(), |d, (_, label)| {
                d.child(
                    div()
                        .absolute()
                        .bottom_2()
                        .left_3()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .bg(crate::settings::color("raised"))
                        .text_color(hsla(vt::theme().fg))
                        .text_xs()
                        .child(*label),
                )
            })
    }
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}
