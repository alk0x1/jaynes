//! A build for tests that draws its windows into pictures (JAYNES_APP_SHOT) does it unseen, so a test never shows on
//! the person's screen: no Dock icon and no menu bar (the app is an accessory), no window ever ordered onto a screen,
//! no keyboard taken, no menu bar item, and nothing asked of the system (no keychain item, no Touch ID, no notice).
//! The pictures need none of it: each is drawn into a texture off every screen (`render_current_to_image`).
//!
//! Before each picture the app checks that this still holds: no window of its own visible, none of its process among
//! the windows on a screen (the system's window list, read by process alone: no window's name, which would take the
//! screen recording permission), the app not in front, and nothing asked. If anything fails, it takes no picture, says
//! why, and ends.
//!
//! With JAYNES_APP_SEEN too, a test sees the app as the person would: its window on the screen, in front and with the
//! keys, so real keys and clicks reach it (a caret blinks only in a window that has them). The pictures are still drawn
//! the same way, with no screen recording, and it still asks nothing of the system: no keychain, Touch ID, notice or
//! menu bar item.

use std::sync::Mutex;

/// Whether this app draws its windows into pictures, asking nothing of the system.
pub fn on() -> bool {
    crate::testing::var("JAYNES_APP_SHOT").is_some()
}

/// What a test build drawing pictures may do on the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Not drawing pictures.
    Off,
    /// Off every screen: no window shown, never in front, no keys taken.
    Hidden,
    /// Its window on the screen, in front and with the keys.
    Seen,
}

/// The mode from the two variables: JAYNES_APP_SHOT draws pictures, JAYNES_APP_SEEN lets the window show.
pub fn mode_of(shot: bool, seen: bool) -> Mode {
    match (shot, seen) {
        (false, _) => Mode::Off,
        (true, false) => Mode::Hidden,
        (true, true) => Mode::Seen,
    }
}

/// This app's mode, from its variables.
pub fn mode() -> Mode {
    mode_of(crate::testing::var("JAYNES_APP_SHOT").is_some(), crate::testing::var("JAYNES_APP_SEEN").is_some())
}

/// Whether this app keeps off every screen: a test drawing pictures that was not told it may be seen.
pub fn hidden() -> bool {
    mode() == Mode::Hidden
}

/// Why the app may not take a picture now: what it tried to ask of the system, or in `Hidden`, a window of its own
/// on a screen or the app in front (`window`, read only then).
pub fn breach_of(asked: Option<&str>, mode: Mode, window: impl FnOnce() -> Option<String>) -> Option<String> {
    if let Some(what) = asked {
        return Some(format!("it tried to ask the system for {what}"));
    }
    if mode == Mode::Hidden { window() } else { None }
}

static ASKED: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Something the app would have asked of the system (the keychain, Touch ID, a notice), not asked: the test is told.
pub fn refused(what: &str) {
    tracing::warn!("unseen: {what} not asked: a test app drawing pictures asks nothing of the system");
    ASKED.lock().unwrap_or_else(|e| e.into_inner()).push(what.to_owned());
}

/// Why the app may not take a picture now, if it may not.
pub fn breach() -> Option<String> {
    let asked = ASKED.lock().unwrap_or_else(|e| e.into_inner()).first().cloned();
    breach_of(asked.as_deref(), mode(), platform::breach)
}

#[cfg(target_os = "macos")]
mod platform {
    use core_foundation::array::{CFArray, CFArrayRef};
    use core_foundation::base::{CFType, TCFType};
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFString;
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSRunningApplication};

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> CFArrayRef;
    }
    /// kCGWindowListOptionOnScreenOnly: the windows on a screen now.
    const ON_SCREEN_ONLY: u32 = 1;

    /// How many windows of process `pid` are on a screen, as the system lists them.
    pub fn on_screen(pid: i64) -> usize {
        // SAFETY: a copy the caller owns (the create rule), or null when the system gives none.
        let list = unsafe { CGWindowListCopyWindowInfo(ON_SCREEN_ONLY, 0) };
        if list.is_null() {
            return 0;
        }
        // SAFETY: a CFArray of CFDictionary, as the call's contract says; wrapped under the create rule, released once.
        let list: CFArray<CFDictionary<CFString, CFType>> = unsafe { CFArray::wrap_under_create_rule(list) };
        let owner = CFString::from_static_string("kCGWindowOwnerPID");
        list.iter().filter(|w| w.find(&owner).and_then(|v| v.downcast::<CFNumber>()).and_then(|n| n.to_i64()) == Some(pid)).count()
    }

    pub fn breach() -> Option<String> {
        let mtm = MainThreadMarker::new()?;
        let app = NSApplication::sharedApplication(mtm);
        let visible = app.windows().iter().filter(|w| w.isVisible()).count();
        if visible > 0 {
            return Some(format!("{visible} of its windows are visible"));
        }
        let pid = std::process::id() as i64;
        let shown = on_screen(pid);
        if shown > 0 {
            return Some(format!("the system lists {shown} of its windows on a screen"));
        }
        if NSRunningApplication::currentApplication().isActive() {
            return Some("it is the app in front".into());
        }
        None
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    pub fn breach() -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{Mode, breach_of, mode_of};

    #[test]
    fn seen_the_window_may_show_and_take_the_keys_but_the_app_still_asks_nothing() {
        assert!(crate::testing::VARIABLES.contains(&"JAYNES_APP_SEEN"), "a test's variable, read by no build for people");
        assert_eq!(mode_of(false, false), Mode::Off);
        // SEEN alone draws nothing
        assert_eq!(mode_of(false, true), Mode::Off);
        assert_eq!(mode_of(true, false), Mode::Hidden);
        assert_eq!(mode_of(true, true), Mode::Seen);
        // a window on the screen, in front: no breach when seen
        assert_eq!(breach_of(None, Mode::Seen, || Some("1 of its windows are visible".into())), None);
        // asking the keychain still ends it
        assert_eq!(breach_of(Some("a keychain item"), Mode::Seen, || None).as_deref(), Some("it tried to ask the system for a keychain item"));
        // hidden, as before: a window on a screen ends it
        assert_eq!(breach_of(None, Mode::Hidden, || Some("1 of its windows are visible".into())).as_deref(), Some("1 of its windows are visible"));
    }

    #[test]
    fn what_the_app_tried_to_ask_is_a_breach() {
        // this test alone asks; the list lives as long as the process
        super::refused("a keychain item");
        assert_eq!(super::breach().as_deref(), Some("it tried to ask the system for a keychain item"));
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn the_test_process_has_no_window_on_a_screen() {
        // a test binary is no app: the system lists none of its windows, and reading the list needs no permission
        assert_eq!(super::platform::on_screen(std::process::id() as i64), 0);
    }
}
