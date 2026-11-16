//! JAYNES: copied from herdr-app's src/vt.rs (Apache-2.0) and changed: its tests stay in herdr-app.
//!
//! Herdr's view of a core terminal: libghostty-vt fed with the relayed bytes,
//! read back as frames to draw, and asked to encode keys and mouse input.

use ghostty_sys::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::rc::Rc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CellPosition {
    pub row: u32,
    pub col: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub struct SelectionPositions {
    pub start: CellPosition,
    pub end: CellPosition,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl From<GhosttyColorRgb> for Rgb {
    fn from(c: GhosttyColorRgb) -> Self {
        Rgb(c.r, c.g, c.b)
    }
}

impl Rgb {
    /// WCAG relative luminance.
    fn luminance(self) -> f32 {
        let ch = |v: u8| {
            let v = v as f32 / 255.0;
            if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * ch(self.0) + 0.7152 * ch(self.1) + 0.0722 * ch(self.2)
    }
}

fn contrast(a: Rgb, b: Rgb) -> f32 {
    let (x, y) = (a.luminance(), b.luminance());
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// Ghostty's graphics ranges retain intentional low contrast, as do our sprites.
fn preserves_graphics_color(text: &str) -> bool {
    crate::sprites::is_sprite(text)
        || text.chars().next().is_some_and(|c| matches!(c as u32,
            0x2500..=0x259f | 0xe0b0..=0xe0d7 | 0x1fb00..=0x1fbff | 0x1cc00..=0x1cebf))
}

/// `fg`, or near-white / near-black when it would vanish into `bg`.
fn readable(fg: Rgb, bg: Rgb, minimum: f32) -> Rgb {
    if minimum <= 1.0 || contrast(fg, bg) >= minimum {
        return fg;
    }
    let (light, dark) = (Rgb(0xf2, 0xf2, 0xf4), Rgb(0x1b, 0x1b, 0x1e));
    if contrast(light, bg) >= contrast(dark, bg) { light } else { dark }
}

#[derive(Clone, PartialEq)]
pub struct Theme {
    pub fg: Rgb,
    pub bg: Rgb,
    pub cursor: Rgb,
    pub selection_bg: Rgb,
    pub palette16: [Rgb; 16],
    pub minimum_contrast: f32,
}

/// The terminal colors from settings.json's theme.
pub fn theme() -> Theme {
    use crate::settings::rgb;
    let t = crate::settings::theme();
    let get = |k: &str| rgb(t.get(k).map(String::as_str).unwrap_or("#ff00ff"));
    let mut palette16 = [Rgb::default(); 16];
    for (i, c) in palette16.iter_mut().enumerate() {
        *c = get(&format!("ansi{i}"));
    }
    Theme { fg: get("terminal_foreground"), bg: get("background"), cursor: get("cursor"), selection_bg: get("selection"), palette16, minimum_contrast: crate::settings::get().minimum_contrast }
}

#[derive(Clone, Default, PartialEq)]
pub struct Cell {
    /// Empty for blank cells and for the tail half of a wide character.
    pub text: String,
    pub fg: Rgb,
    pub bg: Option<Rgb>,
    pub bold: bool,
    pub italic: bool,
    pub faint: bool,
    pub underline: Option<Underline>,
    /// The underline's own color (SGR 58); `None` draws it in the text's color.
    pub underline_color: Option<Rgb>,
    pub strike: bool,
    pub wide: bool,
}

/// How a cell is underlined (SGR 4, 4:1 to 4:5, 21).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Underline {
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CursorShape {
    Block,
    Bar,
    Underline,
    Hollow,
}

#[derive(Clone, Copy)]
pub struct Cursor {
    pub x: u16,
    pub y: u16,
    pub shape: CursorShape,
    pub color: Rgb,
}

pub struct Frame {
    /// A row that didn't change since the last frame is the same `Rc`.
    pub rows: Vec<Rc<Vec<Cell>>>,
    /// Every row changed (output scrolled, a resize, new colors): none is kept.
    pub all: bool,
    pub cursor: Option<Cursor>,
    /// The cursor's cell even when the program hides it (agents draw their
    /// own): where input methods put their candidate window.
    pub cursor_cell: Option<(u16, u16)>,
    pub bg: Rgb,
    /// Search highlights on the current viewport, already converted from
    /// Ghostty's short-lived grid references into cell coordinates.
    pub search_spans: Vec<SearchSpan>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SearchSpan {
    pub row: u16,
    pub start: u16,
    pub end: u16,
    pub selected: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SearchProgress {
    pub total: usize,
    /// Zero-based index in Ghostty's newest-to-oldest ordering.
    pub selected: Option<usize>,
    pub complete: bool,
}

impl Default for SearchProgress {
    fn default() -> Self {
        Self { total: 0, selected: None, complete: true }
    }
}

/// Holds what the terminal's callbacks write into; boxed so its address is stable.
struct Shared {
    state: GhosttyRenderState,
    size: GhosttySizeReportSize,
    held_since: Option<Instant>,
    title_changed: bool,
    /// Replies the terminal writes back to the program (e.g. kitty graphics acks).
    replies: Vec<u8>,
    clipboard_write: bool,
    clipboard: [Option<String>; 2],
}

/// One kitty graphics image on screen, in cells of the viewport.
#[derive(Clone, Debug, PartialEq)]
pub struct Placement {
    pub image: u32,
    /// Changes whenever the image's pixels do (cache key).
    pub generation: u64,
    pub col: i32,
    pub row: i32,
    pub width: u32,
    pub height: u32,
    pub source: [u32; 4],
    pub offset: [u32; 2],
    /// Placeholder geometry uses integer cells; the view may use fractional cells.
    pub cell_pixels: Option<[u32; 2]>,
    pub z: i32,
}

impl Placement {
    pub fn pixel_geometry(&self, cell_width: f32, cell_height: f32) -> [f32; 4] {
        let [sx, sy] = self.cell_pixels.map_or([1.0, 1.0], |[w, h]| [cell_width / w as f32, cell_height / h as f32]);
        [
            self.col as f32 * cell_width + self.offset[0] as f32 * sx,
            self.row as f32 * cell_height + self.offset[1] as f32 * sy,
            self.width as f32 * sx,
            self.height as f32 * sy,
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct KittyVirtualPlacementSpec {
    image_id: u32,
    placement_id: u32,
    columns: u32,
    rows: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct KittyVirtualCell {
    x: u16,
    y: u16,
    image_id_low: u32,
    image_id_high: Option<u32>,
    placement_id: u32,
    row: Option<u32>,
    col: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct KittyVirtualRun {
    x: u16,
    y: u16,
    image_id_low: u32,
    image_id_high: Option<u32>,
    placement_id: u32,
    row: u32,
    col: u32,
    width: u32,
}

static KITTY_PLACEHOLDER_DIACRITICS: OnceLock<HashMap<u32, u32>> = OnceLock::new();

fn kitty_virtual_cell(x: u16, y: u16, grid_ref: &GhosttyGridRef) -> Option<KittyVirtualCell> {
    let mut graphemes = vec![0u32; 4];
    let mut len = 0;
    let mut style = GhosttyStyle { size: std::mem::size_of::<GhosttyStyle>(), ..Default::default() };
    unsafe {
        let result = ghostty_grid_ref_graphemes(grid_ref, graphemes.as_mut_ptr(), graphemes.len(), &mut len);
        if result == GHOSTTY_OUT_OF_SPACE {
            graphemes.resize(len, 0);
            if ghostty_grid_ref_graphemes(grid_ref, graphemes.as_mut_ptr(), graphemes.len(), &mut len) != GHOSTTY_SUCCESS {
                return None;
            }
        } else if result != GHOSTTY_SUCCESS {
            return None;
        }
        if len == 0 || graphemes[0] != 0x10eeee {
            return None;
        }
        if ghostty_grid_ref_style(grid_ref, &mut style) != GHOSTTY_SUCCESS {
            return None;
        }
    }
    graphemes.truncate(len);
    Some(KittyVirtualCell {
        x,
        y,
        image_id_low: kitty_placeholder_color_to_id(style.fg_color),
        image_id_high: graphemes.get(3).and_then(|c| kitty_placeholder_diacritic_index(*c)).filter(|high| *high <= 255),
        placement_id: kitty_placeholder_color_to_id(style.underline_color),
        row: graphemes.get(1).and_then(|c| kitty_placeholder_diacritic_index(*c)),
        col: graphemes.get(2).and_then(|c| kitty_placeholder_diacritic_index(*c)),
    })
}

fn kitty_placeholder_color_to_id(color: GhosttyStyleColor) -> u32 {
    unsafe {
        match color.tag {
            GHOSTTY_STYLE_COLOR_PALETTE => u32::from(color.value.palette),
            GHOSTTY_STYLE_COLOR_RGB => {
                let rgb = color.value.rgb;
                (u32::from(rgb.r) << 16) | (u32::from(rgb.g) << 8) | u32::from(rgb.b)
            }
            _ => 0,
        }
    }
}

fn kitty_virtual_placement_geometry(
    run: KittyVirtualRun,
    spec: KittyVirtualPlacementSpec,
    image_width: u32,
    image_height: u32,
    cell_width: u32,
    cell_height: u32,
    generation: u64,
) -> Option<Placement> {
    if image_width == 0 || image_height == 0 {
        return None;
    }
    let cols = if spec.columns == 0 { image_width.div_ceil(cell_width) } else { spec.columns };
    let rows = if spec.rows == 0 { image_height.div_ceil(cell_height) } else { spec.rows };
    if run.col >= cols || run.row >= rows {
        return None;
    }
    let (grid_width, grid_height) = (f64::from(cols) * f64::from(cell_width), f64::from(rows) * f64::from(cell_height));
    let scale = (grid_width / f64::from(image_width)).min(grid_height / f64::from(image_height));
    let (image_left, image_top) = ((grid_width - f64::from(image_width) * scale) / 2.0, (grid_height - f64::from(image_height) * scale) / 2.0);
    let (left, top) = (f64::from(run.col) * f64::from(cell_width), f64::from(run.row) * f64::from(cell_height));
    let (right, bottom) = (f64::from((run.col + run.width).min(cols)) * f64::from(cell_width), top + f64::from(cell_height));
    let (x0, y0) = (left.max(image_left), top.max(image_top));
    let (x1, y1) = (right.min(image_left + f64::from(image_width) * scale), bottom.min(image_top + f64::from(image_height) * scale));
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let sx0 = (((x0 - image_left) / scale).round() as u32).min(image_width - 1);
    let sy0 = (((y0 - image_top) / scale).round() as u32).min(image_height - 1);
    let sx1 = (((x1 - image_left) / scale).round() as u32).clamp(sx0 + 1, image_width);
    let sy1 = (((y1 - image_top) / scale).round() as u32).clamp(sy0 + 1, image_height);
    // Round shared edges, not slice lengths, so neighboring runs meet exactly.
    let width = (x1.round() - x0.round()) as u32;
    let height = (y1.round() - y0.round()) as u32;
    if width == 0 || height == 0 {
        return None;
    }
    Some(Placement {
        image: spec.image_id,
        generation,
        col: i32::from(run.x),
        row: i32::from(run.y),
        width,
        height,
        source: [sx0, sy0, sx1 - sx0, sy1 - sy0],
        offset: [(x0.round() - left) as u32, (y0.round() - top) as u32],
        cell_pixels: Some([cell_width, cell_height]),
        // Ghostty gives placeholder images a fixed layer, ignoring the virtual placement's z.
        z: -1,
    })
}

fn kitty_placeholder_diacritic_index(codepoint: u32) -> Option<u32> {
    let map = KITTY_PLACEHOLDER_DIACRITICS.get_or_init(|| {
        // Reuse Ghostty's vendored table so Herdr decodes the same placeholder
        // row/column diacritics that libghostty accepts.
        let source =
            include_str!("../vendor/ghostty/src/terminal/kitty/graphics_unicode.zig");
        let mut map = HashMap::new();
        let mut in_table = false;
        for line in source.lines() {
            let line = line.trim();
            if line.starts_with("const diacritics:") {
                in_table = true;
                continue;
            }
            if !in_table {
                continue;
            }
            if line == "};" {
                break;
            }
            let Some(hex) = line
                .strip_prefix("0x")
                .and_then(|value| value.strip_suffix(','))
            else {
                continue;
            };
            if let Ok(value) = u32::from_str_radix(hex, 16) {
                map.insert(value, map.len() as u32);
            }
        }
        map
    });
    map.get(&codepoint).copied()
}


impl KittyVirtualRun {
    fn from_cell(cell: KittyVirtualCell) -> Self {
        Self {
            x: cell.x,
            y: cell.y,
            image_id_low: cell.image_id_low,
            image_id_high: cell.image_id_high,
            placement_id: cell.placement_id,
            row: cell.row.unwrap_or(0),
            col: cell.col.unwrap_or(0),
            width: 1,
        }
    }

    fn append(&mut self, cell: KittyVirtualCell) -> bool {
        if self.image_id_low != cell.image_id_low
            || self.placement_id != cell.placement_id
            || cell.row.is_some_and(|row| row != self.row)
            || cell.col.is_some_and(|col| col != self.col + self.width)
            || cell
                .image_id_high
                .is_some_and(|high| Some(high) != self.image_id_high)
        {
            return false;
        }
        self.width += 1;
        true
    }

    fn image_id(self) -> u32 {
        self.image_id_low | (self.image_id_high.unwrap_or(0) << 24)
    }

}

unsafe extern "C" fn on_write_pty(_t: GhosttyTerminal, userdata: *mut c_void, data: *const u8, len: usize) {
    let shared = unsafe { &mut *(userdata as *mut Shared) };
    if !data.is_null() && len > 0 {
        shared.replies.extend_from_slice(unsafe { std::slice::from_raw_parts(data, len) });
    }
}

const MAX_CLIPBOARD_BYTES: usize = 1 << 20;

unsafe extern "C" fn on_clipboard_write(_t: GhosttyTerminal, userdata: *mut c_void, request: *const GhosttyClipboardWrite) {
    let shared = unsafe { &mut *(userdata as *mut Shared) };
    let request = unsafe { &*request };
    if !shared.clipboard_write || request.size < std::mem::size_of::<GhosttyClipboardWrite>() {
        return;
    }
    let slot = match request.location {
        GHOSTTY_CLIPBOARD_LOCATION_STANDARD => 0,
        GHOSTTY_CLIPBOARD_LOCATION_PRIMARY if cfg!(target_os = "linux") => 1,
        _ => return,
    };
    let text = if request.contents_len == 0 {
        ""
    } else {
        if request.contents_len != 1 || request.contents.is_null() {
            return;
        }
        let content = unsafe { &*request.contents };
        if content.mime.ptr.is_null() || content.data.len > MAX_CLIPBOARD_BYTES {
            return;
        }
        let mime = unsafe { std::slice::from_raw_parts(content.mime.ptr, content.mime.len) };
        if mime != b"text/plain" {
            return;
        }
        let bytes = if content.data.len == 0 {
            &[][..]
        } else if content.data.ptr.is_null() {
            return;
        } else {
            unsafe { std::slice::from_raw_parts(content.data.ptr, content.data.len) }
        };
        let Ok(text) = std::str::from_utf8(bytes) else { return };
        text
    };
    // Keep only the last write per destination, bounding a whole output batch.
    shared.clipboard[slot] = Some(text.to_owned());
    if let Some(reply) = request.reply {
        let result = GhosttyClipboardWriteReply {
            size: std::mem::size_of::<GhosttyClipboardWriteReply>(),
            result: GHOSTTY_CLIPBOARD_WRITE_RESULT_SUCCESS,
            remember: false,
        };
        unsafe { reply(request, &result) };
    }
}

/// PNG decoding for kitty graphics (libghostty-vt leaves it to the embedder).
unsafe extern "C" fn decode_png(_ud: *mut c_void, alloc: *const GhosttyAllocator, data: *const u8, len: usize, out: *mut GhosttySysImage) -> bool {
    let bytes = unsafe { std::slice::from_raw_parts(data, len) };
    let mut decoder = png::Decoder::new(bytes);
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16 | png::Transformations::ALPHA);
    let Ok(mut reader) = decoder.read_info() else { return false };
    let mut buf = vec![0; reader.output_buffer_size()];
    let Ok(info) = reader.next_frame(&mut buf) else { return false };
    let (w, h) = (info.width, info.height);
    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => buf[..info.buffer_size()].to_vec(),
        png::ColorType::GrayscaleAlpha => buf[..info.buffer_size()].chunks(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        png::ColorType::Rgb => buf[..info.buffer_size()].chunks(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        png::ColorType::Grayscale => buf[..info.buffer_size()].iter().flat_map(|g| [*g, *g, *g, 255]).collect(),
        _ => return false,
    };
    let ptr = unsafe { ghostty_alloc(alloc, rgba.len()) };
    if ptr.is_null() {
        return false;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(rgba.as_ptr(), ptr, rgba.len());
        *out = GhosttySysImage { width: w, height: h, data: ptr, data_len: rgba.len() };
    }
    true
}

fn install_png_decoder() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        let f: GhosttySysDecodePngFn = Some(decode_png);
        ghostty_sys_set(GHOSTTY_SYS_OPT_DECODE_PNG, std::mem::transmute::<GhosttySysDecodePngFn, *const c_void>(f));
    });
}

pub struct Vt {
    term: GhosttyTerminal,
    /// Created lazily when a find bar opens; each terminal owns its own search.
    search: Option<GhosttySearch>,
    search_active: bool,
    search_progress: SearchProgress,
    search_spans: Vec<SearchSpan>,
    shared: Box<Shared>,
    rows_it: GhosttyRenderStateRowIterator,
    cells: GhosttyRenderStateRowCells,
    keys: GhosttyKeyEncoder,
    key_event: GhosttyKeyEvent,
    mouse: GhosttyMouseEncoder,
    mouse_event: GhosttyMouseEvent,
    last_mouse_report: Vec<u8>,
    pub cols: u16,
    pub rows: u16,
    theme: Theme,
    /// The built-in 256-color palette, which the theme's 16 colors override.
    base_palette: [GhosttyColorRgb; 256],
    /// Ghostty's own click/double-click/triple-click/drag selection logic.
    gesture: GhosttySelectionGesture,
    press: GhosttySelectionGestureEvent,
    drag: GhosttySelectionGestureEvent,
    release: GhosttySelectionGestureEvent,
    /// The cursor, the kitty images and the colors as last drawn: changes to
    /// them don't mark the render state dirty.
    drawn: (CursorKey, u64, u64),
    /// The rows read last frame by libghostty's row id, with their selection:
    /// one that isn't dirty is the same row and isn't read again.
    seen: HashMap<[u64; 2], (Rc<Vec<Cell>>, Option<(u16, u16)>)>,
    /// The width and colors they were read with: a change reads every row.
    seen_with: (u16, u64),
    placements_cache: RefCell<Option<Vec<Placement>>>,
    /// Whether the program wants win32 key events (Ghostty doesn't track it).
    pub win32: crate::win32_keys::Win32Input,
}

/// What of the cursor a frame shows: (on screen, x, y, visible, style).
type CursorKey = (bool, u16, u16, bool, i32);

/// Where a link shows: (viewport row, first column, last column) per row.
pub type Spans = Vec<(u16, u16, u16)>;

/// A character of link text and the cell it shows in.
#[derive(Clone)]
struct Spot {
    y: u32,
    x: u16,
    width: u16,
    byte: usize,
    uri: Option<String>,
}

/// A screen row's text, where each character shows, and whether the
/// terminal wrapped it onto the next.
struct Row {
    text: String,
    spots: Vec<Spot>,
    wraps: bool,
}

/// The text of `rows` and where each character shows. Where a row doesn't
/// wrap, it loses its trailing blanks and the next row its leading ones, so
/// a path a program broke across them reads whole.
fn link_text(rows: &[Row]) -> (String, Vec<Spot>) {
    let mut text = String::new();
    let mut spots = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        let blank = |s: &Spot| row.text[s.byte..].starts_with(' ');
        let start = if i > 0 && !rows[i - 1].wraps { row.spots.iter().position(|s| !blank(s)).unwrap_or(row.spots.len()) } else { 0 };
        let end = if i + 1 < rows.len() && !row.wraps { row.spots.iter().rposition(|s| !blank(s)).map_or(0, |e| e + 1) } else { row.spots.len() };
        if start >= end {
            continue;
        }
        let from = row.spots[start].byte;
        let to = row.spots.get(end).map_or(row.text.len(), |s| s.byte);
        let base = text.len();
        text.push_str(&row.text[from..to]);
        spots.extend(row.spots[start..end].iter().map(|s| Spot { byte: base + s.byte - from, ..s.clone() }));
    }
    (text, spots)
}

impl Row {
    /// Whether the row runs on into the next: the terminal wrapped it, or it
    /// reaches the right edge, where a program breaks a line too long to fit.
    fn full(&self, cols: u16) -> bool {
        // Cells a program may leave blank at the edge of its own width.
        const EDGE: u16 = 4;
        self.wraps || self.spots.iter().rev().find(|s| !self.text[s.byte..].starts_with(' ')).is_some_and(|s| s.x + s.width + EDGE >= cols)
    }
}

/// The run of text without whitespace at byte `at`; empty on whitespace.
fn word_range(text: &str, at: usize) -> std::ops::Range<usize> {
    if text[at..].starts_with(char::is_whitespace) {
        return at..at;
    }
    let start = text[..at].rfind(char::is_whitespace).map_or(0, |i| i + text[i..].chars().next().map_or(1, char::len_utf8));
    let end = text[at..].find(char::is_whitespace).map_or(text.len(), |i| at + i);
    start..end
}

/// A link found in the terminal: what it says, and where it shows.
#[derive(Clone, Debug, PartialEq)]
pub struct LinkHit {
    pub target: String,
    pub spans: Spans,
}

fn hyperlink_uri(r: &GhosttyGridRef) -> Option<String> {
    let mut len = 0usize;
    let mut buf = vec![0u8; 512];
    let mut res = unsafe { ghostty_grid_ref_hyperlink_uri(r, buf.as_mut_ptr(), buf.len(), &mut len) };
    if res == GHOSTTY_OUT_OF_SPACE {
        buf.resize(len, 0);
        res = unsafe { ghostty_grid_ref_hyperlink_uri(r, buf.as_mut_ptr(), buf.len(), &mut len) };
    }
    (res == GHOSTTY_SUCCESS && len > 0).then(|| String::from_utf8_lossy(&buf[..len]).into_owned())
}

fn is_wide(r: &GhosttyGridRef) -> bool {
    let mut cell: GhosttyCell = Default::default();
    let mut wide: GhosttyCellWide = GHOSTTY_CELL_WIDE_NARROW;
    let read = unsafe {
        ghostty_grid_ref_cell(r, &mut cell) == GHOSTTY_SUCCESS
            && ghostty_cell_get(cell, GHOSTTY_CELL_DATA_WIDE, &mut wide as *mut _ as *mut c_void) == GHOSTTY_SUCCESS
    };
    read && wide == GHOSTTY_CELL_WIDE_WIDE
}

#[derive(Clone, Copy)]
pub enum Select {
    Press,
    Drag,
    Release,
}

fn ok(r: GhosttyResult, what: &str) {
    assert!(r == GHOSTTY_SUCCESS, "{what} failed: {r}");
}

unsafe extern "C" fn on_render_hold(_t: GhosttyTerminal, userdata: *mut c_void, held: bool) {
    let shared = unsafe { &mut *(userdata as *mut Shared) };
    if held {
        // Capture the finished frame the program wants on screen before it redraws.
        unsafe { ghostty_render_state_update(shared.state, _t) };
        shared.held_since = Some(Instant::now());
    } else {
        shared.held_since = None;
    }
}

unsafe extern "C" fn on_size(_t: GhosttyTerminal, userdata: *mut c_void, out: *mut GhosttySizeReportSize) -> bool {
    let shared = unsafe { &*(userdata as *const Shared) };
    if shared.size.cell_width == 0 || shared.size.cell_height == 0 {
        return false;
    }
    unsafe { *out = shared.size };
    true
}

unsafe extern "C" fn on_title(_t: GhosttyTerminal, userdata: *mut c_void) {
    let shared = unsafe { &mut *(userdata as *mut Shared) };
    shared.title_changed = true;
}

const SYNC_OUTPUT: GhosttyMode = 2026;
/// On from the start, as in Ghostty: VS16 makes ✍️ two cells wide and in color.
const GRAPHEME_CLUSTER: GhosttyMode = 2027;
const BRACKETED_PASTE: GhosttyMode = 2004;
const MAX_HOLD: Duration = Duration::from_millis(1000);
const SCROLLBACK_MAX_LINES: usize = 10_000;
// Bound memory per terminal; wide or heavily styled rows may hit this first.
const SCROLLBACK_MAX_BYTES: usize = 10_000_000;
/// Units per pixel for selection gestures (see `select`).
const SUBPIXEL: f32 = 64.0;

impl Vt {
    pub fn new(cols: u16, rows: u16) -> Vt {
        unsafe {
            let mut term = std::ptr::null_mut();
            ok(ghostty_terminal_new(std::ptr::null(), &mut term, cols.max(1), rows.max(1)), "terminal_new");
            let cluster = GhosttyTerminalModeConfig { mode: GRAPHEME_CLUSTER, value: true };
            ghostty_terminal_set(term, GHOSTTY_TERMINAL_OPT_MODE_DEFAULT, &cluster as *const _ as *const c_void);
            let mut state = std::ptr::null_mut();
            ok(ghostty_render_state_new(std::ptr::null(), &mut state), "render_state_new");
            let mut rows_it = std::ptr::null_mut();
            ok(ghostty_render_state_row_iterator_new(std::ptr::null(), &mut rows_it), "row_iterator_new");
            let mut cells = std::ptr::null_mut();
            ok(ghostty_render_state_row_cells_new(std::ptr::null(), &mut cells), "row_cells_new");
            let mut keys = std::ptr::null_mut();
            ok(ghostty_key_encoder_new(std::ptr::null(), &mut keys), "key_encoder_new");
            let mut key_event = std::ptr::null_mut();
            ok(ghostty_key_event_new(std::ptr::null(), &mut key_event), "key_event_new");
            let mut mouse = std::ptr::null_mut();
            ok(ghostty_mouse_encoder_new(std::ptr::null(), &mut mouse), "mouse_encoder_new");
            let mut mouse_event = std::ptr::null_mut();
            ok(ghostty_mouse_event_new(std::ptr::null(), &mut mouse_event), "mouse_event_new");

            install_png_decoder();
            let mut shared = Box::new(Shared { state, size: GhosttySizeReportSize { rows: rows.max(1), columns: cols.max(1), cell_width: 0, cell_height: 0 }, held_since: None, title_changed: false, replies: Vec::new(), clipboard_write: true, clipboard: [None, None] });
            let ud = &mut *shared as *mut Shared as *const c_void;
            ghostty_terminal_set(term, GHOSTTY_TERMINAL_OPT_USERDATA, ud);
            let hold: GhosttyTerminalRenderHoldFn = Some(on_render_hold);
            ghostty_terminal_set(term, GHOSTTY_TERMINAL_OPT_RENDER_HOLD, std::mem::transmute(hold));
            let title: GhosttyTerminalTitleChangedFn = Some(on_title);
            ghostty_terminal_set(term, GHOSTTY_TERMINAL_OPT_TITLE_CHANGED, std::mem::transmute(title));
            let size: GhosttyTerminalSizeFn = Some(on_size);
            ghostty_terminal_set(term, GHOSTTY_TERMINAL_OPT_SIZE, std::mem::transmute(size));
            let write: GhosttyTerminalWritePtyFn = Some(on_write_pty);
            ghostty_terminal_set(term, GHOSTTY_TERMINAL_OPT_WRITE_PTY, std::mem::transmute(write));
            let clipboard: GhosttyTerminalClipboardWriteFn = Some(on_clipboard_write);
            ghostty_terminal_set(term, GHOSTTY_TERMINAL_OPT_CLIPBOARD_WRITE, std::mem::transmute::<GhosttyTerminalClipboardWriteFn, *const c_void>(clipboard));
            let max_clipboard: usize = MAX_CLIPBOARD_BYTES;
            ghostty_terminal_set(term, GHOSTTY_TERMINAL_OPT_CLIPBOARD_WRITE_MAX_BYTES, &max_clipboard as *const _ as *const c_void);
            // No read callback: programs can never read the local clipboard.
            let images: u64 = 128 << 20;
            ghostty_terminal_set(term, GHOSTTY_TERMINAL_OPT_KITTY_IMAGE_STORAGE_LIMIT, &images as *const _ as *const c_void);

            ghostty_render_state_update(state, term);
            let mut colors = GhosttyRenderStateColors { size: std::mem::size_of::<GhosttyRenderStateColors>(), ..Default::default() };
            ghostty_render_state_get(state, GHOSTTY_RENDER_STATE_DATA_COLORS, &mut colors as *mut _ as *mut c_void);

            ok(ghostty_terminal_set(term, GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_BYTES, &SCROLLBACK_MAX_BYTES as *const _ as *const c_void), "scrollback_max_bytes");
            ok(ghostty_terminal_set(term, GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_LINES, &SCROLLBACK_MAX_LINES as *const _ as *const c_void), "scrollback_max_lines");

            let mut gesture = std::ptr::null_mut();
            ok(ghostty_selection_gesture_new(std::ptr::null(), &mut gesture), "selection_gesture_new");
            let ev = |t| {
                let mut e = std::ptr::null_mut();
                ok(ghostty_selection_gesture_event_new(std::ptr::null(), &mut e, t), "gesture_event_new");
                e
            };
            let (press, drag, release) = (
                ev(GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_PRESS),
                ev(GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_DRAG),
                ev(GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_RELEASE),
            );
            let mut vt = Vt {
                term,
                search: None,
                search_active: false,
                search_progress: SearchProgress::default(),
                search_spans: Vec::new(),
                shared,
                rows_it,
                cells,
                keys,
                key_event,
                mouse,
                last_mouse_report: Vec::new(),
                mouse_event,
                cols,
                rows,
                theme: theme(),
                base_palette: colors.palette,
                gesture,
                press,
                drag,
                release,
                drawn: Default::default(),
                seen: HashMap::new(),
                seen_with: (0, 0),
                placements_cache: RefCell::new(None),
                win32: Default::default(),
            };
            vt.apply_theme();
            vt
        }
    }

    /// Switches colors live (settings.json changed).
    pub fn set_theme(&mut self, theme: Theme) {
        if self.theme != theme {
            self.theme = theme;
            self.apply_theme();
            // Rows carry the theme's own colors too (the selection's).
            self.seen.clear();
        }
    }

    fn apply_theme(&mut self) {
        let c = |rgb: Rgb| GhosttyColorRgb { r: rgb.0, g: rgb.1, b: rgb.2 };
        let t = &self.theme;
        let (fg, bg, cur) = (c(t.fg), c(t.bg), c(t.cursor));
        let mut palette = self.base_palette;
        for (i, rgb) in t.palette16.iter().enumerate() {
            palette[i] = c(*rgb);
        }
        unsafe {
            ghostty_terminal_set(self.term, GHOSTTY_TERMINAL_OPT_COLOR_FOREGROUND, &fg as *const _ as *const c_void);
            ghostty_terminal_set(self.term, GHOSTTY_TERMINAL_OPT_COLOR_BACKGROUND, &bg as *const _ as *const c_void);
            ghostty_terminal_set(self.term, GHOSTTY_TERMINAL_OPT_COLOR_CURSOR, &cur as *const _ as *const c_void);
            ghostty_terminal_set(self.term, GHOSTTY_TERMINAL_OPT_COLOR_PALETTE, &palette as *const _ as *const c_void);
        }
    }


    /// What the terminal wants to say back to the program since the last call.
    pub fn take_replies(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.shared.replies)
    }

    /// Visible kitty graphics placements, bottom layer first.
    pub fn placements(&self) -> Vec<Placement> {
        if let Some(placements) = self.placements_cache.borrow().as_ref() {
            return placements.clone();
        }
        let mut out = Vec::new();
        unsafe {
            let mut g: GhosttyKittyGraphics = std::ptr::null_mut();
            if ghostty_terminal_get(self.term, GHOSTTY_TERMINAL_DATA_KITTY_GRAPHICS, &mut g as *mut _ as *mut c_void) != GHOSTTY_SUCCESS || g.is_null() {
                return out;
            }
            let mut it: GhosttyKittyGraphicsPlacementIterator = std::ptr::null_mut();
            if ghostty_kitty_graphics_placement_iterator_new(std::ptr::null(), &mut it) != GHOSTTY_SUCCESS {
                return out;
            }
            let mut specs = Vec::new();
            if ghostty_kitty_graphics_get(g, GHOSTTY_KITTY_GRAPHICS_DATA_PLACEMENT_ITERATOR, &mut it as *mut _ as *mut c_void) == GHOSTTY_SUCCESS {
                while ghostty_kitty_graphics_placement_next(it) {
                    let (mut id, mut z, mut virt) = (0u32, 0i32, false);
                    ghostty_kitty_graphics_placement_get(it, GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_IMAGE_ID, &mut id as *mut _ as *mut c_void);
                    ghostty_kitty_graphics_placement_get(it, GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_Z, &mut z as *mut _ as *mut c_void);
                    ghostty_kitty_graphics_placement_get(it, GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_IS_VIRTUAL, &mut virt as *mut _ as *mut c_void);
                    let img = ghostty_kitty_graphics_image(g, id);
                    if virt {
                        let (mut placement_id, mut columns, mut rows) = (0u32, 0u32, 0u32);
                        ghostty_kitty_graphics_placement_get(it, GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_PLACEMENT_ID, &mut placement_id as *mut _ as *mut c_void);
                        ghostty_kitty_graphics_placement_get(it, GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_COLUMNS, &mut columns as *mut _ as *mut c_void);
                        ghostty_kitty_graphics_placement_get(it, GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_ROWS, &mut rows as *mut _ as *mut c_void);
                        specs.push(KittyVirtualPlacementSpec { image_id: id, placement_id, columns, rows });
                        continue;
                    }
                    if img.is_null() {
                        continue;
                    }
                    let mut info = GhosttyKittyGraphicsPlacementRenderInfo { size: std::mem::size_of::<GhosttyKittyGraphicsPlacementRenderInfo>(), ..Default::default() };
                    if ghostty_kitty_graphics_placement_render_info(it, img, self.term, &mut info) != GHOSTTY_SUCCESS || !info.viewport_visible {
                        continue;
                    }
                    let mut generation = 0u64;
                    ghostty_kitty_graphics_image_get(img, GHOSTTY_KITTY_IMAGE_DATA_GENERATION, &mut generation as *mut _ as *mut c_void);
                    out.push(Placement {
                        image: id,
                        generation,
                        col: info.viewport_col,
                        row: info.viewport_row,
                        width: info.pixel_width,
                        height: info.pixel_height,
                        source: [info.source_x, info.source_y, info.source_width, info.source_height],
                        offset: [0, 0],
                        cell_pixels: None,
                        z,
                    });
                }
            }
            ghostty_kitty_graphics_placement_iterator_free(it);
            out.extend(self.kitty_virtual_image_placements(g, &specs));
        }
        out.sort_by_key(|p| p.z);
        *self.placements_cache.borrow_mut() = Some(out.clone());
        out
    }

    fn kitty_virtual_image_placements(&self, graphics: GhosttyKittyGraphics, specs: &[KittyVirtualPlacementSpec]) -> Vec<Placement> {
        if specs.is_empty() {
            return Vec::new();
        }
        let (mut width, mut height) = (0u32, 0u32);
        unsafe {
            ghostty_terminal_get(self.term, GHOSTTY_TERMINAL_DATA_WIDTH_PX, &mut width as *mut _ as *mut c_void);
            ghostty_terminal_get(self.term, GHOSTTY_TERMINAL_DATA_HEIGHT_PX, &mut height as *mut _ as *mut c_void);
        }
        let cell_width = (width / u32::from(self.cols.max(1))).max(1);
        let cell_height = (height / u32::from(self.rows.max(1))).max(1);
        let mut runs = Vec::new();
        for y in 0..self.rows {
            let Some(first) = self.grid_ref(GHOSTTY_POINT_TAG_VIEWPORT, 0, u32::from(y)) else { continue };
            let mut row = GhosttyRow::default();
            let mut placeholders = false;
            unsafe {
                ghostty_grid_ref_row(&first, &mut row);
                ghostty_row_get(row, GHOSTTY_ROW_DATA_KITTY_VIRTUAL_PLACEHOLDER, &mut placeholders as *mut _ as *mut c_void);
            }
            if !placeholders {
                continue;
            }
            let mut current: Option<KittyVirtualRun> = None;
            for x in 0..self.cols {
                let cell = self.grid_ref(GHOSTTY_POINT_TAG_VIEWPORT, x, u32::from(y))
                    .and_then(|grid_ref| kitty_virtual_cell(x, y, &grid_ref));
                match cell {
                    Some(cell) => {
                        if let Some(run) = current.as_mut() {
                            if run.append(cell) {
                                continue;
                            }
                            runs.push(*run);
                        }
                        current = Some(KittyVirtualRun::from_cell(cell));
                    }
                    None => {
                        if let Some(run) = current.take() {
                            runs.push(run);
                        }
                    }
                }
            }
            if let Some(run) = current {
                runs.push(run);
            }
        }
        let mut placements = Vec::new();
        for run in runs {
            let id = run.image_id();
            let Some(spec) = specs.iter().find(|spec| spec.image_id == id
                && (run.placement_id == 0 || spec.placement_id == run.placement_id)) else { continue };
            let image = unsafe { ghostty_kitty_graphics_image(graphics, id) };
            if image.is_null() {
                continue;
            }
            let (mut image_width, mut image_height, mut generation) = (0u32, 0u32, 0u64);
            unsafe {
                ghostty_kitty_graphics_image_get(image, GHOSTTY_KITTY_IMAGE_DATA_WIDTH, &mut image_width as *mut _ as *mut c_void);
                ghostty_kitty_graphics_image_get(image, GHOSTTY_KITTY_IMAGE_DATA_HEIGHT, &mut image_height as *mut _ as *mut c_void);
                ghostty_kitty_graphics_image_get(image, GHOSTTY_KITTY_IMAGE_DATA_GENERATION, &mut generation as *mut _ as *mut c_void);
            }
            let Some(placement) = kitty_virtual_placement_geometry(run, *spec, image_width, image_height, cell_width, cell_height, generation) else { continue };

            // Extend the slice ending right above with the same source columns: one
            // sprite per image, as sprites meeting at a row edge show seams.
            let bottom = |p: &Placement| i64::from(p.row) * i64::from(cell_height) + i64::from(p.offset[1]) + i64::from(p.height);
            let top = i64::from(placement.row) * i64::from(cell_height) + i64::from(placement.offset[1]);
            let above = placements.iter_mut().find(|p: &&mut Placement| (p.image, p.generation, p.col, p.width, p.offset[0], p.cell_pixels)
                    == (placement.image, placement.generation, placement.col, placement.width, placement.offset[0], placement.cell_pixels)
                && (p.source[0], p.source[2], p.source[1] + p.source[3]) == (placement.source[0], placement.source[2], placement.source[1])
                && bottom(p) == top);
            match above {
                Some(p) => {
                    p.height += placement.height;
                    p.source[3] += placement.source[3];
                }
                None => placements.push(placement),
            }
        }
        placements
    }

    /// An image's pixels as BGRA (what GPUI uploads), cropped to `source`.
    pub fn image_bgra(&self, id: u32, source: [u32; 4]) -> Option<(u32, u32, Vec<u8>)> {
        unsafe {
            let mut g: GhosttyKittyGraphics = std::ptr::null_mut();
            ghostty_terminal_get(self.term, GHOSTTY_TERMINAL_DATA_KITTY_GRAPHICS, &mut g as *mut _ as *mut c_void);
            let img = (!g.is_null()).then(|| ghostty_kitty_graphics_image(g, id)).filter(|i| !i.is_null())?;
            let (mut w, mut h, mut fmt) = (0u32, 0u32, GHOSTTY_KITTY_IMAGE_FORMAT_RGBA);
            let (mut ptr, mut len): (*const u8, usize) = (std::ptr::null(), 0);
            ghostty_kitty_graphics_image_get(img, GHOSTTY_KITTY_IMAGE_DATA_WIDTH, &mut w as *mut _ as *mut c_void);
            ghostty_kitty_graphics_image_get(img, GHOSTTY_KITTY_IMAGE_DATA_HEIGHT, &mut h as *mut _ as *mut c_void);
            ghostty_kitty_graphics_image_get(img, GHOSTTY_KITTY_IMAGE_DATA_FORMAT, &mut fmt as *mut _ as *mut c_void);
            if ghostty_kitty_graphics_image_get(img, GHOSTTY_KITTY_IMAGE_DATA_DATA_PTR, &mut ptr as *mut _ as *mut c_void) != GHOSTTY_SUCCESS || ptr.is_null() {
                return None;
            }
            ghostty_kitty_graphics_image_get(img, GHOSTTY_KITTY_IMAGE_DATA_DATA_LEN, &mut len as *mut _ as *mut c_void);
            let px = std::slice::from_raw_parts(ptr, len);
            let bpp = match fmt {
                GHOSTTY_KITTY_IMAGE_FORMAT_RGB => 3,
                GHOSTTY_KITTY_IMAGE_FORMAT_GRAY_ALPHA => 2,
                GHOSTTY_KITTY_IMAGE_FORMAT_GRAY => 1,
                _ => 4,
            };
            let [sx, sy, sw, sh] = source;
            let (sw, sh) = (sw.min(w.saturating_sub(sx)), sh.min(h.saturating_sub(sy)));
            if sw == 0 || sh == 0 {
                return None;
            }
            let mut out = Vec::with_capacity((sw * sh * 4) as usize);
            for y in sy..sy + sh {
                for x in sx..sx + sw {
                    let i = ((y * w + x) as usize) * bpp;
                    let p = px.get(i..i + bpp)?;
                    let (r, g, b, a) = match bpp {
                        4 => (p[0], p[1], p[2], p[3]),
                        3 => (p[0], p[1], p[2], 255),
                        2 => (p[0], p[0], p[0], p[1]),
                        _ => (p[0], p[0], p[0], 255),
                    };
                    out.extend_from_slice(&[b, g, r, a]);
                }
            }
            Some((sw, sh, out))
        }
    }

    /// Feeds one mouse step of a selection gesture (cell + pixel position in the grid).
    /// Applies the resulting selection; a plain click clears it.
    pub fn select(&mut self, step: Select, col: u16, row: u16, x: f32, y: f32, cell_w: f32, height: f32) {
        unsafe {
            let mut r = GhosttyGridRef { size: std::mem::size_of::<GhosttyGridRef>(), ..Default::default() };
            let mut p = GhosttyPoint { tag: GHOSTTY_POINT_TAG_VIEWPORT, ..Default::default() };
            p.value.coordinate = GhosttyPointCoordinate { x: col.min(self.cols.saturating_sub(1)), y: row.min(self.rows.saturating_sub(1)) as u32 };
            if ghostty_terminal_grid_ref(self.term, p, &mut r) != GHOSTTY_SUCCESS {
                return;
            }
            // Ghostty takes the cell width as a whole number: at zoom a cell is
            // 9.75 px, and 9 put "the middle of a cell" in the wrong place, so
            // the highlight jumped between cells while dragging. Positions and
            // geometry go in 1/64 px, where the width stays exact.
            let pos = GhosttySurfacePosition { x: (x * SUBPIXEL) as f64, y: (y * SUBPIXEL) as f64 };
            let e = match step {
                Select::Press => self.press,
                Select::Drag => self.drag,
                Select::Release => self.release,
            };
            ghostty_selection_gesture_event_set(e, GHOSTTY_SELECTION_GESTURE_EVENT_OPT_REF, &r as *const _ as *const c_void);
            ghostty_selection_gesture_event_set(e, GHOSTTY_SELECTION_GESTURE_EVENT_OPT_POSITION, &pos as *const _ as *const c_void);
            match step {
                Select::Press => {
                    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0);
                    ghostty_selection_gesture_event_set(e, GHOSTTY_SELECTION_GESTURE_EVENT_OPT_TIME_NS, &now as *const _ as *const c_void);
                    // GPUI doesn't expose the platform's double-click interval.
                    let interval = 500_000_000u64;
                    let distance = f64::from(4.0 * SUBPIXEL);
                    ghostty_selection_gesture_event_set(e, GHOSTTY_SELECTION_GESTURE_EVENT_OPT_REPEAT_INTERVAL_NS, &interval as *const _ as *const c_void);
                    ghostty_selection_gesture_event_set(e, GHOSTTY_SELECTION_GESTURE_EVENT_OPT_REPEAT_DISTANCE, &distance as *const _ as *const c_void);
                }
                Select::Drag => {
                    let g = GhosttySelectionGestureGeometry {
                        columns: self.cols as u32,
                        cell_width: (cell_w.max(1.0) * SUBPIXEL).round() as u32,
                        padding_left: 0,
                        screen_height: (height.max(1.0) * SUBPIXEL).round() as u32,
                    };
                    ghostty_selection_gesture_event_set(e, GHOSTTY_SELECTION_GESTURE_EVENT_OPT_GEOMETRY, &g as *const _ as *const c_void);
                }
                Select::Release => {}
            }
            let mut sel = GhosttySelection { size: std::mem::size_of::<GhosttySelection>(), ..Default::default() };
            match ghostty_selection_gesture_event(self.gesture, self.term, e, &mut sel) {
                GHOSTTY_SUCCESS => {
                    ghostty_terminal_set(self.term, GHOSTTY_TERMINAL_OPT_SELECTION, &sel as *const _ as *const c_void);
                }
                _ if matches!(step, Select::Press) => {
                    ghostty_terminal_set(self.term, GHOSTTY_TERMINAL_OPT_SELECTION, std::ptr::null());
                }
                _ => {}
            }
        }
    }

    fn cursor_position(&self) -> CellPosition {
        let (mut col, mut row) = (0u16, 0u16);
        unsafe {
            ghostty_terminal_get(self.term, GHOSTTY_TERMINAL_DATA_CURSOR_X, &mut col as *mut _ as *mut c_void);
            ghostty_terminal_get(self.term, GHOSTTY_TERMINAL_DATA_CURSOR_Y, &mut row as *mut _ as *mut c_void);
        }
        let reference = self.grid_ref(GHOSTTY_POINT_TAG_ACTIVE, col, row as u32).expect("cursor is in the active screen");
        self.cell_position(&reference).expect("cursor is in the screen")
    }

    fn cell_position(&self, reference: &GhosttyGridRef) -> Option<CellPosition> {
        let mut point = GhosttyPointCoordinate::default();
        (unsafe { ghostty_terminal_point_from_grid_ref(self.term, reference, GHOSTTY_POINT_TAG_SCREEN, &mut point) } == GHOSTTY_SUCCESS)
            .then_some(CellPosition { row: point.y, col: point.x })
    }

    fn selection(&self) -> Option<GhosttySelection> {
        let mut selection = GhosttySelection { size: std::mem::size_of::<GhosttySelection>(), ..Default::default() };
        (unsafe { ghostty_terminal_get(self.term, GHOSTTY_TERMINAL_DATA_SELECTION, &mut selection as *mut _ as *mut c_void) } == GHOSTTY_SUCCESS)
            .then_some(selection)
    }

    pub fn selection_positions(&self) -> Option<SelectionPositions> {
        let selection = self.selection()?;
        Some(SelectionPositions { start: self.cell_position(&selection.start)?, end: self.cell_position(&selection.end)? })
    }

    pub fn select_at(&mut self, at: Option<CellPosition>, mode: &str) -> Result<(), String> {
        if !matches!(mode, "cell" | "word" | "line") {
            return Err("terminal.select mode is cell, word or line".into());
        }
        let at = at.unwrap_or_else(|| self.cursor_position());
        let reference = self.grid_ref(GHOSTTY_POINT_TAG_SCREEN, at.col, at.row).ok_or("terminal.select at is outside the screen and history")?;
        let mut selection = GhosttySelection {
            size: std::mem::size_of::<GhosttySelection>(), start: reference, end: reference, ..Default::default()
        };
        let result = unsafe {
            match mode {
                "word" => {
                    let options = GhosttyTerminalSelectWordOptions { size: std::mem::size_of::<GhosttyTerminalSelectWordOptions>(), ref_: reference, ..Default::default() };
                    ghostty_terminal_select_word(self.term, &options, &mut selection)
                }
                "line" => {
                    let options = GhosttyTerminalSelectLineOptions { size: std::mem::size_of::<GhosttyTerminalSelectLineOptions>(), ref_: reference, semantic_prompt_boundary: true, ..Default::default() };
                    ghostty_terminal_select_line(self.term, &options, &mut selection)
                }
                _ => GHOSTTY_SUCCESS,
            }
        };
        if result == GHOSTTY_NO_VALUE {
            self.clear_selection();
            return Ok(());
        }
        if result != GHOSTTY_SUCCESS {
            return Err("terminal.select could not select that cell".into());
        }
        unsafe { ghostty_terminal_set(self.term, GHOSTTY_TERMINAL_OPT_SELECTION, &selection as *const _ as *const c_void); }
        Ok(())
    }

    pub fn adjust_selection(&mut self, direction: &str) -> Result<(), String> {
        let adjustment = match direction {
            "left" => GHOSTTY_SELECTION_ADJUST_LEFT,
            "right" => GHOSTTY_SELECTION_ADJUST_RIGHT,
            "up" => GHOSTTY_SELECTION_ADJUST_UP,
            "down" => GHOSTTY_SELECTION_ADJUST_DOWN,
            "page_up" => GHOSTTY_SELECTION_ADJUST_PAGE_UP,
            "page_down" => GHOSTTY_SELECTION_ADJUST_PAGE_DOWN,
            "home" => GHOSTTY_SELECTION_ADJUST_HOME,
            "end" => GHOSTTY_SELECTION_ADJUST_END,
            "line_start" => GHOSTTY_SELECTION_ADJUST_BEGINNING_OF_LINE,
            "line_end" => GHOSTTY_SELECTION_ADJUST_END_OF_LINE,
            _ => return Err("terminal.adjust_selection direction is left, right, up, down, page_up, page_down, home, end, line_start or line_end".into()),
        };
        let mut selection = self.selection().ok_or("terminal.adjust_selection needs a selection")?;
        if unsafe { ghostty_terminal_selection_adjust(self.term, &mut selection, adjustment) } != GHOSTTY_SUCCESS {
            return Err("terminal.adjust_selection could not move the selection".into());
        }
        // Convert before installing the snapshot: any mutating call invalidates its refs.
        let end = self.cell_position(&selection.end).ok_or("selection end is outside the screen")?;
        unsafe { ghostty_terminal_set(self.term, GHOSTTY_TERMINAL_OPT_SELECTION, &selection as *const _ as *const c_void); }
        let scrollbar = self.scrollbar();
        let row = u64::from(end.row);
        if row < scrollbar.offset {
            self.scroll_to_row(row as usize);
        } else if row >= scrollbar.offset.saturating_add(scrollbar.len) {
            self.scroll_to_row(row.saturating_sub(scrollbar.len.saturating_sub(1)) as usize);
        }
        Ok(())
    }

    /// Select the active screen and its history without formatting or copying text.
    pub fn select_all(&mut self) {
        let mut selection = GhosttySelection { size: std::mem::size_of::<GhosttySelection>(), ..Default::default() };
        unsafe {
            if ghostty_terminal_select_all(self.term, &mut selection) == GHOSTTY_SUCCESS {
                ghostty_terminal_set(self.term, GHOSTTY_TERMINAL_OPT_SELECTION, &selection as *const _ as *const c_void);
            } else {
                self.clear_selection();
            }
        }
    }

    /// The selected text, if anything is selected.
    pub fn selection_text(&self) -> Option<String> {
        unsafe {
            let opts = GhosttyTerminalSelectionFormatOptions {
                size: std::mem::size_of::<GhosttyTerminalSelectionFormatOptions>(),
                emit: GHOSTTY_FORMATTER_FORMAT_PLAIN,
                unwrap: true,
                trim: true,
                ..Default::default()
            };
            let mut ptr: *mut u8 = std::ptr::null_mut();
            let mut len = 0usize;
            if ghostty_terminal_selection_format_alloc(self.term, std::ptr::null(), opts, &mut ptr, &mut len) != GHOSTTY_SUCCESS {
                return None;
            }
            if ptr.is_null() {
                return None;
            }
            let text = String::from_utf8_lossy(std::slice::from_raw_parts(ptr, len)).into_owned();
            ghostty_free(std::ptr::null(), ptr, len);
            (!text.is_empty()).then_some(text)
        }
    }

    fn grid_ref(&self, tag: GhosttyPointTag, x: u16, y: u32) -> Option<GhosttyGridRef> {
        let mut r = GhosttyGridRef { size: std::mem::size_of::<GhosttyGridRef>(), ..Default::default() };
        let mut p = GhosttyPoint { tag, ..Default::default() };
        p.value.coordinate = GhosttyPointCoordinate { x, y };
        (unsafe { ghostty_terminal_grid_ref(self.term, p, &mut r) } == GHOSTTY_SUCCESS).then_some(r)
    }

    /// The link at a viewport cell: a link the program marked (OSC 8), or a
    /// URL or path in the text. Follows lines the terminal wrapped, also into
    /// rows scrolled out of view. Then tries the lines around joined, their
    /// indentation dropped: programs like Claude Code break long paths onto
    /// new lines themselves. `find` is told when its text holds lines joined; a
    /// link found there wins when it's longer.
    pub fn link_at(&self, col: u16, row: u16, mut find: impl FnMut(&str, usize, bool) -> Option<std::ops::Range<usize>>) -> Option<LinkHit> {
        if col >= self.cols || row >= self.rows {
            return None;
        }
        // Screen rows count from the top of the scrollback; the viewport starts at `top`.
        let top = unsafe {
            let r = self.grid_ref(GHOSTTY_POINT_TAG_VIEWPORT, 0, 0)?;
            let mut p = GhosttyPointCoordinate::default();
            if ghostty_terminal_point_from_grid_ref(self.term, &r, GHOSTTY_POINT_TAG_SCREEN, &mut p) != GHOSTTY_SUCCESS {
                return None;
            }
            p.y
        };
        let hovered = top + row as u32;
        // A URL longer than this many rows isn't one worth finding.
        const REACH: u32 = 32;
        let mut first = hovered;
        while first > 0 && hovered - first < REACH && self.wraps(first - 1) {
            first -= 1;
        }
        let mut last = hovered;
        while last - hovered < REACH && self.wraps(last) && self.grid_ref(GHOSTTY_POINT_TAG_SCREEN, 0, last + 1).is_some() {
            last += 1;
        }
        let mut rows: Vec<Row> = (first..=last).map_while(|y| self.link_row(y)).collect();
        let (text, spots) = link_text(&rows);
        let at = spots.iter().position(|s| s.y == hovered && s.x <= col && col < s.x + s.width)?;
        let hit = self.link_in(&text, &spots, top, at, &mut find, false);
        if spots[at].uri.is_some() {
            return hit;
        }

        // Lines a program broke itself, joined: only across rows that reach
        // the right edge, and only where the hovered word runs into one.
        const BROKEN: u32 = 4;
        let word = word_range(&text, spots[at].byte);
        let mut above = Vec::new();
        if text[..word.start].trim().is_empty() {
            for y in (first.saturating_sub(BROKEN)..first).rev() {
                match self.link_row(y) {
                    Some(row) if row.full(self.cols) => above.push(row),
                    _ => break,
                }
            }
        }
        let mut below = Vec::new();
        if text[word.end..].trim().is_empty() && rows.last().is_some_and(|row| row.full(self.cols)) {
            for y in last + 1..=last + BROKEN {
                let Some(row) = self.link_row(y) else { break };
                let full = row.full(self.cols);
                below.push(row);
                if !full {
                    break;
                }
            }
        }
        let (group, up_most, down_most) = (rows.len(), above.len(), below.len());
        rows.splice(0..0, above.into_iter().rev());
        rows.extend(below);
        // Fewest rows first; the longest link wins, so a break next to a
        // slash doesn't stop at the folder before it. A join that leaves the
        // hovered word as it was can't find anything new.
        let mut tried = vec![text[word].to_owned()];
        let mut best: Option<(LinkHit, String, Vec<Spot>, usize)> = None;
        // Whether `find` last saw the link we'd return: it may keep what it saw.
        let mut fresh = true;
        for joined in 1..=up_most + down_most {
            for up in joined.saturating_sub(down_most)..=joined.min(up_most) {
                let start = up_most - up;
                let (text, spots) = link_text(&rows[start..start + group + joined]);
                let Some(at) = spots.iter().position(|s| s.y == hovered && s.x <= col && col < s.x + s.width) else { continue };
                let word = &text[word_range(&text, spots[at].byte)];
                if tried.iter().any(|t| t == word) {
                    continue;
                }
                tried.push(word.to_owned());
                let Some(found) = self.link_in(&text, &spots, top, at, &mut find, true) else { continue };
                let longest = best.as_ref().map(|b| &b.0).or(hit.as_ref()).map_or(0, |h| h.target.len());
                fresh = found.target.len() > longest;
                if fresh {
                    best = Some((found, text, spots, at));
                }
            }
        }
        match best {
            Some((found, ..)) if fresh => Some(found),
            Some((_, text, spots, at)) => self.link_in(&text, &spots, top, at, &mut find, true),
            None if fresh => hit,
            None => self.link_in(&text, &spots, top, at, &mut find, false),
        }
    }

    /// Whether the terminal wrapped screen row `y` onto the next.
    fn wraps(&self, y: u32) -> bool {
        unsafe {
            let Some(r) = self.grid_ref(GHOSTTY_POINT_TAG_SCREEN, 0, y) else { return false };
            let mut line: GhosttyRow = Default::default();
            let mut wrap = false;
            ghostty_grid_ref_row(&r, &mut line) == GHOSTTY_SUCCESS
                && ghostty_row_get(line, GHOSTTY_ROW_DATA_WRAP, &mut wrap as *mut _ as *mut c_void) == GHOSTTY_SUCCESS
                && wrap
        }
    }

    /// The text of screen row `y` and where each character shows.
    fn link_row(&self, y: u32) -> Option<Row> {
        let mut r = self.grid_ref(GHOSTTY_POINT_TAG_SCREEN, 0, y)?;
        let mut text = String::new();
        let mut spots: Vec<Spot> = Vec::new();
        for x in 0..self.cols {
            r.x = x;
            let (wide, linked) = unsafe {
                let mut cell: GhosttyCell = Default::default();
                ghostty_grid_ref_cell(&r, &mut cell);
                let mut wide: GhosttyCellWide = GHOSTTY_CELL_WIDE_NARROW;
                ghostty_cell_get(cell, GHOSTTY_CELL_DATA_WIDE, &mut wide as *mut _ as *mut c_void);
                let mut linked = false;
                ghostty_cell_get(cell, GHOSTTY_CELL_DATA_HAS_HYPERLINK, &mut linked as *mut _ as *mut c_void);
                (wide, linked)
            };
            if wide == GHOSTTY_CELL_WIDE_SPACER_TAIL || wide == GHOSTTY_CELL_WIDE_SPACER_HEAD {
                continue;
            }
            let byte = text.len();
            let mut cps = [0u32; 16];
            let mut len = 0usize;
            unsafe { ghostty_grid_ref_graphemes(&r, cps.as_mut_ptr(), cps.len(), &mut len) };
            text.extend(cps[..len.min(cps.len())].iter().filter_map(|c| char::from_u32(*c)));
            if text.len() == byte {
                text.push(' ');
            }
            let uri = linked.then(|| hyperlink_uri(&r)).flatten();
            let width = if wide == GHOSTTY_CELL_WIDE_WIDE { 2 } else { 1 };
            spots.push(Spot { y, x, width, byte, uri });
        }
        Some(Row { text, spots, wraps: self.wraps(y) })
    }

    /// The link `find` sees in `text` at spot `at`.
    fn link_in(&self, text: &str, spots: &[Spot], top: u32, at: usize, find: &mut impl FnMut(&str, usize, bool) -> Option<std::ops::Range<usize>>, joined: bool) -> Option<LinkHit> {
        let (target, chosen): (String, Vec<&Spot>) = match spots[at].uri.clone() {
            Some(uri) => {
                let same = |s: &&Spot| s.uri.as_deref() == Some(uri.as_str());
                let start = spots[..at].iter().rposition(|s| !same(&s)).map_or(0, |i| i + 1);
                let end = spots[at..].iter().position(|s| !same(&s)).map_or(spots.len(), |i| at + i);
                (uri, spots[start..end].iter().collect())
            }
            None => {
                let range = find(text, spots[at].byte, joined)?;
                (text[range.clone()].to_owned(), spots.iter().filter(|s| range.contains(&s.byte)).collect())
            }
        };
        let mut spans = Spans::new();
        for s in chosen {
            let Some(vy) = s.y.checked_sub(top).filter(|vy| *vy < self.rows as u32) else { continue };
            let end = s.x + s.width - 1;
            match spans.last_mut() {
                Some(last) if last.0 == vy as u16 => last.2 = end,
                _ => spans.push((vy as u16, s.x, end)),
            }
        }
        Some(LinkHit { target, spans })
    }

    pub fn clear_selection(&mut self) {
        unsafe { ghostty_terminal_set(self.term, GHOSTTY_TERMINAL_OPT_SELECTION, std::ptr::null()) };
    }

    pub fn write(&mut self, bytes: &[u8]) {
        self.placements_cache.get_mut().take();
        self.win32.watch(bytes);
        unsafe { ghostty_terminal_vt_write(self.term, bytes.as_ptr(), bytes.len()) };
        self.feed_search();
    }

    pub fn allow_clipboard_write(&mut self, allow: bool) {
        self.shared.clipboard_write = allow;
    }

    pub fn take_clipboard(&mut self) -> [Option<String>; 2] {
        std::mem::take(&mut self.shared.clipboard)
    }

    pub fn resize(&mut self, cols: u16, rows: u16, cell_w: u32, cell_h: u32) {
        if cols == 0 || rows == 0 {
            return;
        }
        self.placements_cache.get_mut().take();
        self.cols = cols;
        self.rows = rows;
        self.shared.size = GhosttySizeReportSize { rows, columns: cols, cell_width: cell_w.max(1), cell_height: cell_h.max(1) };
        unsafe { ghostty_terminal_resize(self.term, cols, rows, cell_w.max(1), cell_h.max(1)) };
        self.feed_search();
    }

    pub fn take_title(&mut self) -> Option<String> {
        if !std::mem::take(&mut self.shared.title_changed) {
            return None;
        }
        let mut s = GhosttyString::default();
        unsafe { ghostty_terminal_get(self.term, GHOSTTY_TERMINAL_DATA_TITLE, &mut s as *mut _ as *mut c_void) };
        if s.ptr.is_null() {
            return Some(String::new());
        }
        let bytes = unsafe { std::slice::from_raw_parts(s.ptr, s.len) };
        Some(String::from_utf8_lossy(bytes).into_owned())
    }

    pub fn kitty_keyboard_flags(&self) -> u8 {
        let mut flags = 0u8;
        unsafe { ghostty_terminal_get(self.term, GHOSTTY_TERMINAL_DATA_KITTY_KEYBOARD_FLAGS, &mut flags as *mut _ as *mut c_void) };
        flags
    }

    fn get_bool(&self, data: GhosttyTerminalData) -> bool {
        let mut v = false;
        unsafe { ghostty_terminal_get(self.term, data, &mut v as *mut _ as *mut c_void) };
        v
    }

    fn mode(&self, mode: GhosttyMode) -> bool {
        let mut cfg = GhosttyTerminalModeConfig { mode, value: false };
        unsafe { ghostty_terminal_get(self.term, GHOSTTY_TERMINAL_DATA_MODE, &mut cfg as *mut _ as *mut c_void) };
        cfg.value
    }

    pub fn mouse_tracking(&self) -> bool {
        self.get_bool(GHOSTTY_TERMINAL_DATA_MOUSE_TRACKING)
    }

    pub fn alt_screen(&self) -> bool {
        let mut screen: GhosttyTerminalScreen = GHOSTTY_TERMINAL_SCREEN_PRIMARY;
        unsafe { ghostty_terminal_get(self.term, GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN, &mut screen as *mut _ as *mut c_void) };
        screen == GHOSTTY_TERMINAL_SCREEN_ALTERNATE
    }

    pub fn scroll_viewport(&mut self, delta: isize) {
        self.placements_cache.get_mut().take();
        let mut s = GhosttyTerminalScrollViewport { tag: GHOSTTY_SCROLL_VIEWPORT_DELTA, ..Default::default() };
        s.value.delta = delta;
        unsafe { ghostty_terminal_scroll_viewport(self.term, s) };
        self.feed_search();
    }

    /// Scrolls so `row` (from the scrollback's top) is the first visible row.
    pub fn scroll_to_row(&mut self, row: usize) {
        self.placements_cache.get_mut().take();
        let mut s = GhosttyTerminalScrollViewport { tag: GHOSTTY_SCROLL_VIEWPORT_ROW, ..Default::default() };
        s.value.row = row;
        unsafe { ghostty_terminal_scroll_viewport(self.term, s) };
        self.feed_search();
    }

    /// What a scrollbar needs: the scrollable area's total rows, the viewport's
    /// offset into it, and the viewport's length.
    pub fn scrollbar(&self) -> GhosttyTerminalScrollbar {
        let mut sb = GhosttyTerminalScrollbar::default();
        unsafe { ghostty_terminal_get(self.term, GHOSTTY_TERMINAL_DATA_SCROLLBAR, &mut sb as *mut _ as *mut c_void) };
        sb
    }

    pub fn scroll_to_bottom(&mut self) {
        self.placements_cache.get_mut().take();
        let s = GhosttyTerminalScrollViewport { tag: GHOSTTY_SCROLL_VIEWPORT_BOTTOM, ..Default::default() };
        unsafe { ghostty_terminal_scroll_viewport(self.term, s) };
        self.feed_search();
    }

    /// Start (or reuse) this terminal's Ghostty search. The handle belongs to
    /// this Vt, so searches never leak between terminal tabs or panes.
    pub fn start_search(&mut self, needle: &str) {
        if self.search.is_none() {
            let mut search = std::ptr::null_mut();
            let result = unsafe { ghostty_search_new(std::ptr::null(), &mut search, self.term) };
            if result != GHOSTTY_SUCCESS || search.is_null() {
                tracing::warn!("terminal search: ghostty_search_new failed: {result:?}");
                return;
            }
            self.search = Some(search);
        }
        self.set_search_needle(needle);
    }

    pub fn set_search_needle(&mut self, needle: &str) {
        let Some(search) = self.search else { return };
        self.search_active = !needle.is_empty();
        let value = GhosttyString { ptr: needle.as_ptr(), len: needle.len() };
        let value = if needle.is_empty() { std::ptr::null() } else { &value as *const _ as *const c_void };
        let result = unsafe { ghostty_search_set(search, GHOSTTY_SEARCH_OPT_NEEDLE, value) };
        if result != GHOSTTY_SUCCESS {
            return;
        }
        if self.search_active {
            self.feed_search();
            self.progress_search();
        } else {
            self.search_progress = SearchProgress::default();
            self.search_spans.clear();
        }
    }

    pub fn clear_search(&mut self) {
        self.set_search_needle("");
    }

    pub fn search_progress(&self) -> SearchProgress {
        self.search_progress
    }

    /// Make one bounded unit of progress. Interactive searches must not call
    /// Ghostty's blocking `run` on the UI thread.
    pub fn progress_search(&mut self) -> bool {
        let Some(search) = self.search.filter(|_| self.search_active) else { return false };
        let before = (self.search_progress, self.search_spans.clone());
        let mut status = GHOSTTY_SEARCH_STATUS_COMPLETE;
        unsafe { ghostty_search_get(search, GHOSTTY_SEARCH_DATA_STATUS, &mut status as *mut _ as *mut c_void); }
        if status == GHOSTTY_SEARCH_STATUS_FEED_REQUIRED {
            self.feed_search();
        }
        let result = unsafe { ghostty_search_tick(search, &mut status) };
        if result == GHOSTTY_SUCCESS {
            self.read_search_data(search, status);
        }
        before != (self.search_progress, self.search_spans.clone())
    }

    /// Navigate only after incremental indexing is complete; the Ghostty
    /// select API catches up synchronously, so calling it early could stall UI.
    pub fn search_navigate(&mut self, previous: bool) -> bool {
        let Some(search) = self.search.filter(|_| self.search_active && self.search_progress.complete) else { return false };
        let option = if previous { GHOSTTY_SEARCH_OPT_SELECT_PREV } else { GHOSTTY_SEARCH_OPT_SELECT_NEXT };
        if unsafe { ghostty_search_set(search, option, std::ptr::null()) } != GHOSTTY_SUCCESS {
            return false;
        }
        self.feed_search();
        true
    }

    fn feed_search(&mut self) {
        let Some(search) = self.search.filter(|_| self.search_active) else { return };
        if unsafe { ghostty_search_feed(search) } != GHOSTTY_SUCCESS {
            return;
        }
        let mut status = GHOSTTY_SEARCH_STATUS_COMPLETE;
        unsafe { ghostty_search_get(search, GHOSTTY_SEARCH_DATA_STATUS, &mut status as *mut _ as *mut c_void); }
        self.read_search_data(search, status);
    }

    fn read_search_data(&mut self, search: GhosttySearch, status: GhosttySearchStatus) {
        let mut total = 0usize;
        let _ = unsafe { ghostty_search_get(search, GHOSTTY_SEARCH_DATA_TOTAL_MATCHES, &mut total as *mut _ as *mut c_void) };
        let mut selected_index = 0usize;
        let selected = unsafe {
            ghostty_search_get(search, GHOSTTY_SEARCH_DATA_SELECTED_INDEX, &mut selected_index as *mut _ as *mut c_void) == GHOSTTY_SUCCESS
        }.then_some(selected_index);
        self.search_progress = SearchProgress { total, selected, complete: status == GHOSTTY_SEARCH_STATUS_COMPLETE };

        // Ghostty returns viewport-only snapshots, not pointers into terminal
        // state. Convert them immediately: any later terminal mutation makes
        // the grid refs stale.
        let mut buffer = GhosttySelectionBuffer { ptr: std::ptr::null_mut(), cap: 0, len: 0 };
        let result = unsafe { ghostty_search_get(search, GHOSTTY_SEARCH_DATA_VIEWPORT_MATCHES, &mut buffer as *mut _ as *mut c_void) };
        let mut matches = Vec::new();
        if result == GHOSTTY_OUT_OF_SPACE && buffer.len > 0 {
            matches = std::iter::repeat_with(|| GhosttySelection { size: std::mem::size_of::<GhosttySelection>(), ..Default::default() })
                .take(buffer.len)
                .collect();
            buffer.ptr = matches.as_mut_ptr();
            buffer.cap = matches.len();
            buffer.len = 0;
            if unsafe { ghostty_search_get(search, GHOSTTY_SEARCH_DATA_VIEWPORT_MATCHES, &mut buffer as *mut _ as *mut c_void) } != GHOSTTY_SUCCESS {
                self.search_spans.clear();
                return;
            }
        } else if result != GHOSTTY_SUCCESS {
            self.search_spans.clear();
            return;
        }

        let mut selected_match = GhosttySelection { size: std::mem::size_of::<GhosttySelection>(), ..Default::default() };
        let has_selected_match = unsafe {
            ghostty_search_get(search, GHOSTTY_SEARCH_DATA_SELECTED_MATCH, &mut selected_match as *mut _ as *mut c_void) == GHOSTTY_SUCCESS
        };
        let mut spans = Vec::new();
        for matched in matches.iter().take(buffer.len) {
            let selected = if has_selected_match {
                let mut equal = false;
                unsafe { ghostty_terminal_selection_equal(self.term, matched, &selected_match, &mut equal); }
                equal
            } else {
                false
            };
            let mut start = GhosttyPointCoordinate::default();
            let mut end = GhosttyPointCoordinate::default();
            let (start_ok, end_ok) = unsafe {
                (
                    ghostty_terminal_point_from_grid_ref(self.term, &matched.start, GHOSTTY_POINT_TAG_VIEWPORT, &mut start),
                    ghostty_terminal_point_from_grid_ref(self.term, &matched.end, GHOSTTY_POINT_TAG_VIEWPORT, &mut end),
                )
            };
            if start_ok != GHOSTTY_SUCCESS || end_ok != GHOSTTY_SUCCESS {
                continue;
            }
            let swapped = (start.y, start.x) > (end.y, end.x);
            if swapped {
                std::mem::swap(&mut start, &mut end);
            }
            // A match ends on a wide character's first cell; cover its second too.
            if is_wide(if swapped { &matched.start } else { &matched.end }) {
                end.x += 1;
            }
            let last_row = end.y.min(self.rows.saturating_sub(1) as u32);
            if start.y >= self.rows as u32 || start.y > last_row {
                continue;
            }
            for row in start.y..=last_row {
                let first = if row == start.y { start.x } else { 0 };
                let last = if row == end.y { end.x } else { self.cols.saturating_sub(1) };
                if first <= last && first < self.cols {
                    spans.push(SearchSpan { row: row as u16, start: first, end: last.min(self.cols.saturating_sub(1)), selected });
                }
            }
        }
        self.search_spans = spans;
    }

    pub fn scroll_to_top(&mut self) {
        self.placements_cache.get_mut().take();
        let s = GhosttyTerminalScrollViewport { tag: GHOSTTY_SCROLL_VIEWPORT_TOP, ..Default::default() };
        unsafe { ghostty_terminal_scroll_viewport(self.term, s) };
        self.feed_search();
    }

    /// Wraps pasted text the way the program asked (bracketed paste or not).
    pub fn paste_bytes(&self, text: &str) -> Vec<u8> {
        if self.mode(BRACKETED_PASTE) {
            let clean = text.replace("\x1b[201~", "");
            [b"\x1b[200~".as_slice(), clean.as_bytes(), b"\x1b[201~"].concat()
        } else {
            text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
        }
    }

    /// Whether output since the last frame changed what's on screen: a title,
    /// or a cursor parked where it was, needs no frame.
    pub fn shows_change(&mut self) -> bool {
        if self.shared.held_since.is_some() {
            return true;
        }
        unsafe {
            let state = self.shared.state;
            ghostty_render_state_update(state, self.term);
            let mut dirty: GhosttyRenderStateDirty = GHOSTTY_RENDER_STATE_DIRTY_FULL;
            ghostty_render_state_get(state, GHOSTTY_RENDER_STATE_DATA_DIRTY, &mut dirty as *mut _ as *mut c_void);
            dirty != GHOSTTY_RENDER_STATE_DIRTY_FALSE || self.drawable() != self.drawn
        }
    }

    /// The cursor and the colors (from the updated render state), and the kitty
    /// images' generation.
    unsafe fn drawable(&self) -> (CursorKey, u64, u64) {
        unsafe {
            let mut cur = GhosttyRenderStateCursor { size: std::mem::size_of::<GhosttyRenderStateCursor>(), ..Default::default() };
            ghostty_render_state_get(self.shared.state, GHOSTTY_RENDER_STATE_DATA_CURSOR, &mut cur as *mut _ as *mut c_void);
            let cursor = (cur.viewport_has_value, cur.viewport_x, cur.viewport_y, cur.visible, cur.visual_style as i32);
            let mut c = GhosttyRenderStateColors { size: std::mem::size_of::<GhosttyRenderStateColors>(), ..Default::default() };
            ghostty_render_state_get(self.shared.state, GHOSTTY_RENDER_STATE_DATA_COLORS, &mut c as *mut _ as *mut c_void);
            let mut h = std::hash::DefaultHasher::new();
            let rgb = |h: &mut std::hash::DefaultHasher, x: GhosttyColorRgb| std::hash::Hash::hash(&(x.r, x.g, x.b), h);
            for x in [c.background, c.foreground].into_iter().chain(c.palette).chain(c.cursor_has_value.then_some(c.cursor)) {
                rgb(&mut h, x);
            }
            let colors = std::hash::Hasher::finish(&h);
            let mut generation = 0u64;
            let mut g: GhosttyKittyGraphics = std::ptr::null_mut();
            if ghostty_terminal_get(self.term, GHOSTTY_TERMINAL_DATA_KITTY_GRAPHICS, &mut g as *mut _ as *mut c_void) == GHOSTTY_SUCCESS && !g.is_null() {
                ghostty_kitty_graphics_get(g, GHOSTTY_KITTY_GRAPHICS_DATA_GENERATION, &mut generation as *mut _ as *mut c_void);
            }
            (cursor, generation, colors)
        }
    }

    pub fn render_hold_deadline(&self) -> Option<Instant> {
        self.shared.held_since.map(|since| since + MAX_HOLD)
    }

    /// Updates the render state (respecting synchronized output) and reads one frame.
    pub fn frame(&mut self) -> Frame {
        unsafe {
            if let Some(since) = self.shared.held_since
                && since.elapsed() > MAX_HOLD
            {
                let cfg = GhosttyTerminalModeConfig { mode: SYNC_OUTPUT, value: false };
                ghostty_terminal_set(self.term, GHOSTTY_TERMINAL_OPT_MODE, &cfg as *const _ as *const c_void);
                self.shared.held_since = None;
            }
            let state = self.shared.state;
            if self.shared.held_since.is_none() {
                ghostty_render_state_update(state, self.term);
            }

            let mut colors = GhosttyRenderStateColors { size: std::mem::size_of::<GhosttyRenderStateColors>(), ..Default::default() };
            ghostty_render_state_get(state, GHOSTTY_RENDER_STATE_DATA_COLORS, &mut colors as *mut _ as *mut c_void);
            let fg_default = Rgb::from(colors.foreground);
            let bg_default = Rgb::from(colors.background);

            // Rows libghostty keeps clean are taken as read last frame (see
            // "Row Identity" in its render.h); a resize or new colors (which
            // don't mark rows dirty) read them all.
            let with = (self.cols, self.drawable().2);
            let mut dirty: GhosttyRenderStateDirty = GHOSTTY_RENDER_STATE_DIRTY_FULL;
            ghostty_render_state_get(state, GHOSTTY_RENDER_STATE_DATA_DIRTY, &mut dirty as *mut _ as *mut c_void);
            let all = dirty == GHOSTTY_RENDER_STATE_DIRTY_FULL || with != self.seen_with;
            let mut seen = HashMap::with_capacity(self.rows as usize);
            let mut rows = Vec::with_capacity(self.rows as usize);
            ghostty_render_state_get(state, GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR, &mut self.rows_it as *mut _ as *mut c_void);
            while ghostty_render_state_row_iterator_next(self.rows_it) {
                let mut sel = GhosttyRenderStateRowSelection { size: std::mem::size_of::<GhosttyRenderStateRowSelection>(), ..Default::default() };
                let selected = ghostty_render_state_row_get(self.rows_it, GHOSTTY_RENDER_STATE_ROW_DATA_SELECTION, &mut sel as *mut _ as *mut c_void)
                    == GHOSTTY_SUCCESS;
                let selection = selected.then_some((sel.start_x, sel.end_x));
                let mut id = GhosttyRenderStateRowId::default();
                let known = ghostty_render_state_row_get(self.rows_it, GHOSTTY_RENDER_STATE_ROW_DATA_ID, &mut id as *mut _ as *mut c_void) == GHOSTTY_SUCCESS;
                // Everything changed: no row is kept (but each is remembered).
                let mut row_dirty = true;
                if !all {
                    ghostty_render_state_row_get(self.rows_it, GHOSTTY_RENDER_STATE_ROW_DATA_DIRTY, &mut row_dirty as *mut _ as *mut c_void);
                }
                let same = (known && !row_dirty).then(|| self.seen.get(&id.bits)).flatten().filter(|(_, s)| *s == selection);
                let row = match same {
                    Some((row, _)) => row.clone(),
                    None => {
                        ghostty_render_state_row_get(self.rows_it, GHOSTTY_RENDER_STATE_ROW_DATA_CELLS, &mut self.cells as *mut _ as *mut c_void);
                        let mut row = Vec::with_capacity(self.cols as usize);
                        let mut x: u16 = 0;
                        while ghostty_render_state_row_cells_next(self.cells) {
                            row.push(self.read_cell(fg_default, &colors.palette, selected && x >= sel.start_x && x <= sel.end_x));
                            x += 1;
                        }
                        Rc::new(row)
                    }
                };
                if known {
                    seen.insert(id.bits, (row.clone(), selection));
                }
                rows.push(row);
            }
            (self.seen, self.seen_with) = (seen, with);

            let mut cur = GhosttyRenderStateCursor { size: std::mem::size_of::<GhosttyRenderStateCursor>(), ..Default::default() };
            ghostty_render_state_get(state, GHOSTTY_RENDER_STATE_DATA_CURSOR, &mut cur as *mut _ as *mut c_void);
            let cursor = (cur.visible && cur.viewport_has_value).then(|| Cursor {
                x: if cur.wide_tail { cur.viewport_x.saturating_sub(1) } else { cur.viewport_x },
                y: cur.viewport_y,
                shape: match cur.visual_style {
                    GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BAR => CursorShape::Bar,
                    GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_UNDERLINE => CursorShape::Underline,
                    GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BLOCK_HOLLOW => CursorShape::Hollow,
                    _ => CursorShape::Block,
                },
                color: if colors.cursor_has_value { colors.cursor.into() } else { fg_default },
            });

            self.drawn = self.drawable();
            ghostty_render_state_clean(state);
            let cursor_cell = cur.viewport_has_value.then_some((cur.viewport_x, cur.viewport_y));
            Frame { rows, all, cursor, cursor_cell, bg: bg_default, search_spans: self.search_spans.clone() }
        }
    }

    unsafe fn read_cell(&mut self, fg_default: Rgb, palette: &[GhosttyColorRgb; 256], selected: bool) -> Cell {
        unsafe {
            let c = self.cells;
            let mut raw: GhosttyCell = Default::default();
            ghostty_render_state_row_cells_get(c, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_RAW, &mut raw as *mut _ as *mut c_void);
            let mut wide: GhosttyCellWide = GHOSTTY_CELL_WIDE_NARROW;
            ghostty_cell_get(raw, GHOSTTY_CELL_DATA_WIDE, &mut wide as *mut _ as *mut c_void);

            let mut style = GhosttyStyle { size: std::mem::size_of::<GhosttyStyle>(), ..Default::default() };
            ghostty_render_state_row_cells_get(c, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE, &mut style as *mut _ as *mut c_void);

            let mut fg = GhosttyColorRgb::default();
            let fg = if ghostty_render_state_row_cells_get(c, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_FG_COLOR, &mut fg as *mut _ as *mut c_void)
                == GHOSTTY_SUCCESS
            {
                Rgb::from(fg)
            } else {
                fg_default
            };
            let mut bg = GhosttyColorRgb::default();
            let bg = (ghostty_render_state_row_cells_get(c, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_BG_COLOR, &mut bg as *mut _ as *mut c_void)
                == GHOSTTY_SUCCESS)
                .then(|| Rgb::from(bg));

            let (mut fg, mut bg) = (fg, bg);
            if style.inverse {
                let new_fg = bg.unwrap_or(self.theme.bg);
                bg = Some(fg);
                fg = new_fg;
            }
            if selected {
                bg = Some(self.theme.selection_bg);
            }
            let mut text = String::new();
            if wide != GHOSTTY_CELL_WIDE_SPACER_TAIL && wide != GHOSTTY_CELL_WIDE_SPACER_HEAD && !style.invisible {
                let mut len: u32 = 0;
                ghostty_render_state_row_cells_get(c, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_LEN, &mut len as *mut _ as *mut c_void);
                if len > 0 {
                    let mut cps = vec![0u32; len as usize];
                    ghostty_render_state_row_cells_get(c, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_BUF, cps.as_mut_ptr() as *mut c_void);
                    text = cps.into_iter().filter_map(char::from_u32).collect();
                }
            }

            if text.starts_with('\u{10eeee}') {
                text.clear();
                style.underline = 0;
                style.strikethrough = false;
            }

            // Contrast adjustment is opt-in; graphics always keep their colors.
            if self.theme.minimum_contrast > 1.0 && !preserves_graphics_color(&text) {
                fg = readable(fg, bg.unwrap_or(self.theme.bg), self.theme.minimum_contrast);
            }

            let underline = match style.underline as GhosttySgrUnderline {
                GHOSTTY_SGR_UNDERLINE_NONE => None,
                GHOSTTY_SGR_UNDERLINE_DOUBLE => Some(Underline::Double),
                GHOSTTY_SGR_UNDERLINE_CURLY => Some(Underline::Curly),
                GHOSTTY_SGR_UNDERLINE_DOTTED => Some(Underline::Dotted),
                GHOSTTY_SGR_UNDERLINE_DASHED => Some(Underline::Dashed),
                _ => Some(Underline::Single),
            };
            // Reverse video swaps the text's colors, not the underline's own.
            // Lazy, so cells without an underline skip the decode.
            let underline_color = underline.and_then(|_| match style.underline_color.tag {
                GHOSTTY_STYLE_COLOR_PALETTE => Some(Rgb::from(palette[style.underline_color.value.palette as usize])),
                GHOSTTY_STYLE_COLOR_RGB => Some(Rgb::from(style.underline_color.value.rgb)),
                _ => None,
            });

            Cell {
                text,
                fg,
                bg,
                bold: style.bold,
                italic: style.italic,
                faint: style.faint,
                underline,
                underline_color,
                strike: style.strikethrough,
                wide: wide == GHOSTTY_CELL_WIDE_WIDE,
            }
        }
    }

    /// Encodes one key press with the program's current keyboard modes.
    /// `consumed`: the modifiers that went into making `text` (Shift for ":",
    /// Option for "\u{df}" on a Mac), as Ghostty reports them. Without it the
    /// kitty keyboard protocol reports shift+; and Neovim never sees ":".
    pub fn encode_key(&mut self, key: GhosttyKey, mods: GhosttyMods, consumed: GhosttyMods, text: Option<&str>, unshifted: Option<char>) -> Vec<u8> {
        unsafe {
            ghostty_key_encoder_setopt_from_terminal(self.keys, self.term);
            #[cfg(target_os = "macos")]
            {
                let option = match gpui_macos::option_as_alt() {
                    gpui_macos::OptionAsAlt::None => GHOSTTY_OPTION_AS_ALT_FALSE,
                    gpui_macos::OptionAsAlt::Both => GHOSTTY_OPTION_AS_ALT_TRUE,
                    gpui_macos::OptionAsAlt::Left => GHOSTTY_OPTION_AS_ALT_LEFT,
                    gpui_macos::OptionAsAlt::Right => GHOSTTY_OPTION_AS_ALT_RIGHT,
                };
                ghostty_key_encoder_setopt(self.keys, GHOSTTY_KEY_ENCODER_OPT_MACOS_OPTION_AS_ALT, &option as *const _ as *const c_void);
            }
            let e = self.key_event;
            ghostty_key_event_set_action(e, GHOSTTY_KEY_ACTION_PRESS);
            ghostty_key_event_set_key(e, key);
            ghostty_key_event_set_mods(e, mods);
            let printable = text.is_some_and(|t| !t.is_empty() && !t.chars().any(|c| c.is_control()));
            ghostty_key_event_set_consumed_mods(e, if printable { consumed } else { 0 });
            match text {
                Some(t) if printable => {
                    ghostty_key_event_set_utf8(e, t.as_ptr() as *const _, t.len())
                }
                _ => ghostty_key_event_set_utf8(e, std::ptr::null(), 0),
            }
            ghostty_key_event_set_unshifted_codepoint(e, unshifted.map_or(0, |c| c as u32));
            let mut buf = [0u8; 128];
            let mut len = 0usize;
            let r = ghostty_key_encoder_encode(self.keys, e, buf.as_mut_ptr() as *mut _, buf.len(), &mut len);
            if r != GHOSTTY_SUCCESS {
                return Vec::new();
            }
            buf[..len].to_vec()
        }
    }

    /// Encodes a mouse event for a program that tracks the mouse. Positions are in pixels.
    #[allow(clippy::too_many_arguments)]
    pub fn encode_mouse(
        &mut self,
        action: GhosttyMouseAction,
        button: Option<GhosttyMouseButton>,
        mods: GhosttyMods,
        x: f32,
        y: f32,
        // (columns, rows, cell width, cell height), cells in px.
        size: (u32, u32, f32, f32),
        any_pressed: bool,
    ) -> Vec<u8> {
        // Ghostty takes whole-pixel cells; at zoom a cell is 11.7 px, and
        // rounding it alone made the reported column run ahead of the pointer
        // (2 cells by mid-line). So the position moves into a grid of whole
        // cells: same column and row exactly, pixels (SGR-pixels mode) within
        // a few percent.
        let (cols, rows, cw, ch) = size;
        let (cwi, chi) = (cw.round().max(1.0), ch.round().max(1.0));
        let (x, y) = (x / cw.max(1.0) * cwi, y / ch.max(1.0) * chi);
        unsafe {
            ghostty_mouse_encoder_setopt_from_terminal(self.mouse, self.term);
            let s = GhosttyMouseEncoderSize {
                size: std::mem::size_of::<GhosttyMouseEncoderSize>(),
                screen_width: cols * cwi as u32,
                screen_height: rows * chi as u32,
                cell_width: cwi as u32,
                cell_height: chi as u32,
                ..Default::default()
            };
            ghostty_mouse_encoder_setopt(self.mouse, GHOSTTY_MOUSE_ENCODER_OPT_SIZE, &s as *const _ as *const c_void);
            ghostty_mouse_encoder_setopt(self.mouse, GHOSTTY_MOUSE_ENCODER_OPT_ANY_BUTTON_PRESSED, &any_pressed as *const _ as *const c_void);
            let e = self.mouse_event;
            ghostty_mouse_event_set_action(e, action);
            match button {
                Some(b) => ghostty_mouse_event_set_button(e, b),
                None => ghostty_mouse_event_clear_button(e),
            }
            ghostty_mouse_event_set_mods(e, mods);
            ghostty_mouse_event_set_position(e, GhosttyMousePosition { x, y });
            let mut buf = [0u8; 64];
            let mut len = 0usize;
            let r = ghostty_mouse_encoder_encode(self.mouse, e, buf.as_mut_ptr() as *mut _, buf.len(), &mut len);
            if r != GHOSTTY_SUCCESS || len == 0 {
                return Vec::new();
            }
            // Refreshing the encoder's terminal/size options resets its cell tracking.
            if action == GHOSTTY_MOUSE_ACTION_MOTION && self.last_mouse_report == buf[..len] {
                return Vec::new();
            }
            let report = buf[..len].to_vec();
            // Remember the motion equivalent of a press/release, so a focus click
            // cannot start a drag at the same cell. Let the encoder handle each
            // format's coordinates, including the finer SGR-pixel positions.
            if action != GHOSTTY_MOUSE_ACTION_MOTION {
                ghostty_mouse_event_set_action(e, GHOSTTY_MOUSE_ACTION_MOTION);
                if action == GHOSTTY_MOUSE_ACTION_RELEASE {
                    ghostty_mouse_event_clear_button(e);
                }
                len = 0;
                if ghostty_mouse_encoder_encode(self.mouse, e, buf.as_mut_ptr() as *mut _, buf.len(), &mut len) != GHOSTTY_SUCCESS {
                    len = 0;
                }
            }
            self.last_mouse_report.clear();
            self.last_mouse_report.extend_from_slice(&buf[..len]);
            report
        }
    }
}

impl Drop for Vt {
    fn drop(&mut self) {
        unsafe {
            if let Some(search) = self.search {
                ghostty_search_free(search);
            }
            ghostty_selection_gesture_event_free(self.press);
            ghostty_selection_gesture_event_free(self.drag);
            ghostty_selection_gesture_event_free(self.release);
            ghostty_selection_gesture_free(self.gesture, self.term);
            ghostty_mouse_event_free(self.mouse_event);
            ghostty_mouse_encoder_free(self.mouse);
            ghostty_key_event_free(self.key_event);
            ghostty_key_encoder_free(self.keys);
            ghostty_render_state_row_cells_free(self.cells);
            ghostty_render_state_row_iterator_free(self.rows_it);
            ghostty_terminal_free(self.term);
            ghostty_render_state_free(self.shared.state);
        }
    }
}
