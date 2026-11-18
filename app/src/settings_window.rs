//! The Settings window (Command-comma, Jaynes › Settings…), the macOS way: a window of its own, its sections in a
//! toolbar at the top, each a list of rows. A change takes effect at once and is kept (prefs.rs); there is no Save.
//! A new setting is one more row in its section's list (`rows`).

use crate::look;
use crate::prefs::{self, Prefs};
use crate::settings::OptionAsAlt;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_base::input::{Input, InputEvent, InputState};
use std::cell::RefCell;

actions!(jaynes_settings, [CloseSettings]);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Section {
    Terminal,
    Notifications,
    Agents,
}

impl Section {
    const ALL: [Section; 3] = [Section::Terminal, Section::Notifications, Section::Agents];

    fn name(self) -> &'static str {
        match self {
            Section::Terminal => "Terminal",
            Section::Notifications => "Notifications",
            Section::Agents => "Agents",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Section::Terminal => r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#000" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="4" width="18" height="16" rx="2"/><path d="M7 9l3 3-3 3M12 15h5"/></svg>"##,
            Section::Notifications => r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#000" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M6 16V11a6 6 0 0 1 12 0v5l2 2H4z"/><path d="M10 21h4"/></svg>"##,
            Section::Agents => r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#000" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="8" r="4"/><path d="M4 21a8 8 0 0 1 16 0"/></svg>"##,
        }
    }

    pub fn named(name: &str) -> Option<Section> {
        Section::ALL.into_iter().find(|s| s.name().eq_ignore_ascii_case(name))
    }
}

/// The terminal fonts offered: the monospace ones the app ships, those of a few others this machine has, and the
/// one in use whatever it is.
pub fn fonts_offered(current: &str) -> Vec<String> {
    const SHIPPED: [&str; 3] = ["IBM Plex Mono", "JetBrains Mono", "Geist Mono"];
    const OTHERS: [&str; 8] = ["SF Mono", "Menlo", "Monaco", "Fira Code", "Berkeley Mono", "Hack", "Source Code Pro", "Cascadia Code"];
    let mut out: Vec<String> = SHIPPED.iter().map(|f| f.to_string()).collect();
    out.extend(OTHERS.iter().filter(|f| crate::settings::has_font(f)).map(|f| f.to_string()));
    if !out.iter().any(|f| f == current) {
        out.push(current.to_owned());
    }
    out
}

const SCROLL_STEP: f32 = 0.25;

pub struct SettingsView {
    section: Section,
    font_open: bool,
    /// When a click outside last closed the font list: the same click on the font itself must not open it again.
    fonts_closed_at: Option<std::time::Instant>,
    new_agent: Entity<InputState>,
    editor: Entity<InputState>,
    focus: FocusHandle,
    note: Option<String>,
}

thread_local! {
    static OPEN: RefCell<Option<WindowHandle<SettingsView>>> = const { RefCell::new(None) };
}

/// Opens the Settings window, or brings it to the front when it is open.
pub fn open(cx: &mut App) {
    if let Some(handle) = OPEN.with(|o| *o.borrow())
        && handle.update(cx, |_, window, _| window.activate_window()).is_ok()
    {
        return;
    }
    let bounds = Bounds::centered(None, size(px(600.), px(500.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions { title: Some("Settings".into()), appears_transparent: true, traffic_light_position: Some(point(px(14.), px(14.))) }),
        is_resizable: false,
        focus: crate::testing::var("JAYNES_APP_SHOT").is_none(),
        show: !crate::unseen::on(),
        ..Default::default()
    };
    let opened = cx.open_window(options, |window, cx| cx.new(|cx| SettingsView::new(window, cx)));
    match opened {
        Ok(handle) => {
            OPEN.with(|o| *o.borrow_mut() = Some(handle));
            // a test sees it as it sees the main window: drawn into a picture beside the main one's
            if let Some(path) = crate::testing::var("JAYNES_APP_SHOT") {
                let path = std::path::PathBuf::from(path.to_string_lossy().replace(".png", "-settings.png"));
                crate::shots(handle.into(), path, cx);
            }
        }
        Err(e) => tracing::warn!("cannot open the Settings window: {e:#}"),
    }
}

/// The open Settings window's view, for the test driver.
pub fn with_open(cx: &mut App, f: impl FnOnce(&mut SettingsView, &mut Window, &mut Context<SettingsView>)) -> bool {
    let Some(handle) = OPEN.with(|o| *o.borrow()) else { return false };
    handle.update(cx, |view, window, cx| f(view, window, cx)).is_ok()
}

impl SettingsView {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let p = prefs::get();
        let new_agent = cx.new(|cx| InputState::new(window, cx).placeholder("claude"));
        new_agent.update(cx, |i, cx| i.set_value(p.new_agent.clone(), window, cx));
        let editor = cx.new(|cx| InputState::new(window, cx).placeholder("$VISUAL, else $EDITOR"));
        editor.update(cx, |i, cx| i.set_value(p.editor.clone(), window, cx));
        // a field's words count once the person is done with them: Return, or leaving the field
        for (input, name) in [(&new_agent, "new_agent"), (&editor, "editor")] {
            cx.subscribe_in(input, window, move |this, input, e: &InputEvent, _, cx| {
                if matches!(e, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                    let value = input.read(cx).value().to_string();
                    this.pick(name, &value, cx);
                }
            })
            .detach();
        }
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        SettingsView { section: Section::Terminal, font_open: false, fonts_closed_at: None, new_agent, editor, focus, note: None }
    }

    /// What a click on a control does: the setting changes, at once, and is kept.
    pub fn pick(&mut self, name: &str, value: &str, cx: &mut Context<Self>) {
        self.font_open = false;
        self.note = prefs::change(name, value, cx).err();
        cx.notify();
    }

    /// The font list, opened or closed as a click on the font does.
    pub fn toggle_fonts(&mut self, cx: &mut Context<Self>) {
        let just_closed = self.fonts_closed_at.take().is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(400));
        self.font_open = !self.font_open && !just_closed;
        cx.notify();
    }

    pub fn show(&mut self, section: Section, cx: &mut Context<Self>) {
        self.section = section;
        self.font_open = false;
        cx.notify();
    }

    fn rows(&self, p: &Prefs, cx: &mut Context<Self>) -> Vec<Div> {
        match self.section {
            Section::Terminal => vec![
                row("Appearance", Some(appearance_hint(p.appearance)), self.appearance(p, cx)),
                row("Colors", Some("The terminal's in the dark; in the light, Linen".into()), self.colors(p, cx)),
                row("Font", None, div().flex().items_center().gap(px(8.)).child(self.font(p, cx)).child(stepper("font-size", format!("{}", p.terminal_font_size), "terminal_font_size", p.terminal_font_size, 1.0, cx))),
                row("Option key", Some(option_hint(p.option_key)), self.option_key(p, cx)),
                row("Copy on select", None, toggle("copy-on-select", p.copy_on_select, "copy_on_select", cx)),
                row("Scroll speed", None, stepper("scroll-speed", times(p.scroll_speed), "scroll_speed", p.scroll_speed, SCROLL_STEP, cx)),
            ],
            Section::Notifications => vec![row(
                "When an agent needs you or finishes",
                Some("Only while Jaynes is not in front".into()),
                toggle("notifications", p.notifications, "notifications", cx),
            )],
            Section::Agents => vec![
                row("New agent runs", Some("What Command-T starts, in the open agent's folder".into()), field(&self.new_agent)),
                row("Editor", Some("What e opens in Changes; empty: your shell's $VISUAL or $EDITOR".into()), field(&self.editor)),
            ],
        }
    }

    fn colors(&self, p: &Prefs, cx: &mut Context<Self>) -> Div {
        let mut sw = div().flex().gap(px(8.));
        for name in prefs::COLORS {
            let [bg, a, b] = crate::settings::swatch(name).unwrap_or([0, 0, 0]);
            let on = p.terminal_colors == *name;
            let label = format!("{}{}", name[..1].to_uppercase(), &name[1..]);
            sw = sw.child(
                div()
                    .id(SharedString::from(format!("colors-{name}")))
                    .w(px(58.))
                    .p(px(5.))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(rgb(if on { look::blue() } else { look::line_2() }))
                    .cursor_pointer()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(4.))
                    .text_size(px(10.5))
                    .text_color(rgb(if on { look::text() } else { look::dim() }))
                    .child(div().w_full().h(px(18.)).rounded(px(3.)).overflow_hidden().flex().child(div().flex_1().bg(rgb(bg))).child(div().flex_1().bg(rgb(a))).child(div().flex_1().bg(rgb(b))))
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| this.pick("terminal_colors", name, cx))),
            );
        }
        sw
    }

    fn font(&self, p: &Prefs, cx: &mut Context<Self>) -> AnyElement {
        let current = p.terminal_font.clone();
        let button = div()
            .id("font")
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(10.))
            .py(px(5.))
            .rounded(px(7.))
            .bg(rgb(look::panel_2()))
            .border_1()
            .border_color(rgb(look::line_2()))
            .cursor_pointer()
            .child(current.clone())
            .child(div().text_color(rgb(look::faint())).text_size(px(10.)).child("▾"))
            .on_click(cx.listener(|this, _, _, cx| this.toggle_fonts(cx)));
        if !self.font_open {
            return button.into_any_element();
        }
        // a click anywhere else closes it, as a menu closes
        let mut list = div()
            .mt(px(4.))
            .min_w(px(180.))
            .py(px(4.))
            .rounded(px(8.))
            .bg(rgb(look::panel_2()))
            .border_1()
            .border_color(rgb(look::line_2()))
            .shadow_lg()
            .occlude()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.font_open = false;
                this.fonts_closed_at = Some(std::time::Instant::now());
                cx.notify();
            }));
        for f in fonts_offered(&current) {
            let on = f == current;
            let value = f.clone();
            list = list.child(
                div()
                    .id(SharedString::from(format!("font-{f}")))
                    .px(px(12.))
                    .py(px(5.))
                    .cursor_pointer()
                    .text_color(rgb(if on { look::text() } else { look::dim() }))
                    .hover(|d| d.bg(rgb(look::selected())).text_color(rgb(look::text())))
                    .child(f)
                    .on_click(cx.listener(move |this, _, _, cx| this.pick("terminal_font", &value, cx))),
            );
        }
        div().child(button).child(deferred(anchored().snap_to_window().child(list)).with_priority(1)).into_any_element()
    }

    fn appearance(&self, p: &Prefs, cx: &mut Context<Self>) -> Div {
        use crate::prefs::Appearance;
        segmented("appearance", &[("Dark", "dark", p.appearance == Appearance::Dark), ("Light", "light", p.appearance == Appearance::Light), ("Match the system", "system", p.appearance == Appearance::System)], cx)
    }

    fn option_key(&self, p: &Prefs, cx: &mut Context<Self>) -> Div {
        let choices = [("Auto", "auto", OptionAsAlt::Auto), ("Alt", "alt", OptionAsAlt::On), ("Left Alt", "left", OptionAsAlt::Left), ("Characters", "characters", OptionAsAlt::Off)];
        let mut seg = div().flex().rounded(px(7.)).border_1().border_color(rgb(look::line_2())).overflow_hidden();
        for (i, (label, value, which)) in choices.into_iter().enumerate() {
            let on = p.option_key == which;
            seg = seg.child(
                div()
                    .id(SharedString::from(format!("option-{value}")))
                    .px(px(10.))
                    .py(px(4.))
                    .cursor_pointer()
                    .when(i > 0, |d| d.border_l_1().border_color(rgb(look::line_2())))
                    .when(on, |d| d.bg(rgb(look::selected())))
                    .text_color(rgb(if on { look::text() } else { look::dim() }))
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| this.pick("option_key", value, cx))),
            );
        }
        seg
    }
}

/// A multiplier as it reads: 1.0×, 1.25×, 0.5×.
fn times(v: f32) -> String {
    let s = format!("{v:.2}");
    let s = s.trim_end_matches('0');
    format!("{}×", if s.ends_with('.') { format!("{s}0") } else { s.to_owned() })
}

/// Segments of one setting, `name`, the one in force lit: (label, value, on).
fn segmented(name: &'static str, choices: &[(&'static str, &'static str, bool)], cx: &mut Context<SettingsView>) -> Div {
    let mut seg = div().flex().rounded(px(7.)).border_1().border_color(rgb(look::line_2())).overflow_hidden();
    for (i, (label, value, on)) in choices.iter().copied().enumerate() {
        seg = seg.child(
            div()
                .id(SharedString::from(format!("{name}-{value}")))
                .px(px(10.))
                .py(px(4.))
                .cursor_pointer()
                .when(i > 0, |d| d.border_l_1().border_color(rgb(look::line_2())))
                .when(on, |d| d.bg(rgb(look::selected())))
                .text_color(rgb(if on { look::text() } else { look::dim() }))
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| this.pick(name, value, cx))),
        );
    }
    seg
}

/// What the appearance picked does, said under its name.
fn appearance_hint(a: crate::prefs::Appearance) -> String {
    use crate::prefs::Appearance;
    match a {
        Appearance::Dark => "Dark, as the window has always been",
        Appearance::Light => "Linen, a warm off-white",
        Appearance::System => "Light or dark as macOS is, changing with it",
    }
    .into()
}

/// What the Option key does, said under its name.
fn option_hint(o: OptionAsAlt) -> String {
    match o {
        OptionAsAlt::Auto => "Auto: Alt on U.S. and ABC keyboards, so Option-arrow moves a word",
        OptionAsAlt::On => "Both Option keys are Alt",
        OptionAsAlt::Left => "The left Option key is Alt; the right one types characters",
        OptionAsAlt::Right => "The right Option key is Alt; the left one types characters",
        OptionAsAlt::Off => "Option types special characters (é, ø, …)",
    }
    .into()
}

/// A setting's row: its name (and a line under it), its control at the right.
fn row(name: &str, hint: Option<String>, control: impl IntoElement) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(14.))
        .py(px(10.))
        .border_b_1()
        .border_color(rgb(look::line()))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(div().text_color(rgb(look::text())).child(name.to_owned()))
                .when_some(hint, |d, h| d.child(div().mt(px(2.)).text_size(px(11.5)).text_color(rgb(look::faint())).child(h))),
        )
        .child(control)
}

fn toggle(id: &'static str, on: bool, name: &'static str, cx: &mut Context<SettingsView>) -> Stateful<Div> {
    div()
        .id(id)
        .w(px(30.))
        .h(px(18.))
        .flex_none()
        .rounded(px(9.))
        .bg(rgb(if on { look::blue() } else { look::line_2() }))
        .cursor_pointer()
        .flex()
        .items_center()
        .px(px(2.))
        .when(on, |d| d.justify_end())
        .child(div().size(px(14.)).rounded_full().bg(rgb(if on { look::on_fill() } else { look::dim() })))
        .on_click(cx.listener(move |this, _, _, cx| this.pick(name, if on { "off" } else { "on" }, cx)))
}

fn stepper(id: &'static str, shown: String, name: &'static str, value: f32, step: f32, cx: &mut Context<SettingsView>) -> Div {
    let button = |which: &'static str, label: &'static str, to: f32, cx: &mut Context<SettingsView>| {
        div()
            .id(SharedString::from(format!("{id}-{which}")))
            .px(px(9.))
            .py(px(3.))
            .cursor_pointer()
            .text_color(rgb(look::faint()))
            .hover(|d| d.text_color(rgb(look::text())))
            .child(label)
            .on_click(cx.listener(move |this, _, _, cx| this.pick(name, &to.to_string(), cx)))
    };
    div()
        .flex()
        .items_center()
        .rounded(px(7.))
        .border_1()
        .border_color(rgb(look::line_2()))
        .child(button("less", "−", value - step, cx))
        .child(div().px(px(10.)).py(px(3.)).min_w(px(44.)).flex().justify_center().border_l_1().border_r_1().border_color(rgb(look::line_2())).font_family(look::MONO_FONT).text_size(px(12.)).child(shown))
        .child(button("more", "+", value + step, cx))
}

fn field(input: &Entity<InputState>) -> Div {
    div().w(px(220.)).h(px(30.)).px(px(10.)).flex().items_center().rounded(px(7.)).bg(rgb(look::panel_2())).border_1().border_color(rgb(look::line_2())).font_family(look::MONO_FONT).text_size(px(12.)).child(Input::new(input))
}

impl Render for SettingsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = prefs::get();
        let mut bar = div().flex().items_end().justify_center().gap(px(4.)).px(px(14.)).pb(px(10.)).border_b_1().border_color(rgb(look::line()));
        for s in Section::ALL {
            let on = s == self.section;
            bar = bar.child(
                div()
                    .id(SharedString::from(format!("section-{}", s.name())))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(4.))
                    .px(px(12.))
                    .py(px(6.))
                    .rounded(px(7.))
                    .cursor_pointer()
                    .text_size(px(11.5))
                    .text_color(rgb(if on { look::text() } else { look::dim() }))
                    .when(on, |d| d.bg(rgb(look::selected())))
                    .hover(|d| d.text_color(rgb(look::text())))
                    .child(svg().data(s.icon().as_bytes()).size(px(18.)).text_color(rgb(if on { look::text() } else { look::dim() })))
                    .child(s.name())
                    .on_click(cx.listener(move |this, _, _, cx| this.show(s, cx))),
            );
        }
        let rows = self.rows(&p, cx);
        let n = rows.len();
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(look::panel()))
            .font_family(look::UI_FONT)
            .text_size(px(12.5))
            .text_color(rgb(look::text()))
            .track_focus(&self.focus)
            .key_context("Settings")
            .on_action(|_: &CloseSettings, window, _| window.remove_window())
            // the title, as macOS draws it: the section shown, between the window's buttons
            .child(
                div()
                    .h(px(38.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .font_weight(FontWeight::SEMIBOLD)
                    .on_mouse_down(MouseButton::Left, |e, window, _| {
                        if e.click_count == 2 {
                            window.titlebar_double_click();
                        } else {
                            window.start_window_move();
                        }
                    })
                    .child(self.section.name()),
            )
            .child(bar)
            .child(div().flex_1().px(px(24.)).py(px(8.)).flex().flex_col().children(rows.into_iter().enumerate().map(|(i, r)| if i + 1 == n { r.border_b_0() } else { r })))
            .when_some(self.note.clone(), |d, n| d.child(div().px(px(24.)).pb(px(12.)).text_size(px(11.5)).text_color(rgb(look::red())).child(n)))
    }
}

#[cfg(test)]
mod tests {
    use super::{Section, fonts_offered, times};

    #[test]
    fn a_multiplier_reads_as_one() {
        assert_eq!((times(1.0).as_str(), times(1.25).as_str(), times(0.5).as_str()), ("1.0×", "1.25×", "0.5×"));
    }

    #[test]
    fn the_fonts_offered_hold_the_shipped_ones_and_the_one_in_use() {
        let fonts = fonts_offered("Comic Mono");
        assert_eq!(fonts[..3], ["IBM Plex Mono", "JetBrains Mono", "Geist Mono"]);
        assert_eq!(fonts.last().map(String::as_str), Some("Comic Mono"));
        assert_eq!(fonts_offered("JetBrains Mono").iter().filter(|f| *f == "JetBrains Mono").count(), 1);
    }

    #[test]
    fn a_section_is_found_by_its_name() {
        assert_eq!(Section::named("notifications"), Some(Section::Notifications));
        assert_eq!(Section::named("Agents"), Some(Section::Agents));
        assert_eq!(Section::named("look"), None);
    }
}
