//! How many stops wait for the person, where they see it without the window: "◆ 2" in the menu bar, and the same
//! count on the app's icon in the Dock (as herdr-app draws its badge). A click on the menu bar brings the window
//! forward on the first run that waits.

use std::sync::atomic::{AtomicBool, Ordering};

/// Set by a click on the menu bar item, taken by the window.
static CLICKED: AtomicBool = AtomicBool::new(false);

/// Whether the person clicked the menu bar item since the last time this was asked.
pub fn clicked() -> bool {
    CLICKED.swap(false, Ordering::Relaxed)
}

/// What a click on the menu bar item does, for a test that cannot click.
pub fn click() {
    CLICKED.store(true, Ordering::Relaxed);
}

/// Shows the count of stops that wait, in the menu bar and on the Dock icon. Runs on the main thread.
pub fn show(waiting: usize) {
    platform::show(waiting);
}

#[cfg(target_os = "macos")]
mod platform {
    use super::CLICKED;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, NSObject};
    use objc2::{define_class, msg_send, sel, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{NSApplication, NSStatusBar, NSStatusItem, NSVariableStatusItemLength};
    use objc2_foundation::NSString;
    use std::cell::RefCell;
    use std::sync::atomic::Ordering;

    define_class!(
        // SAFETY: NSObject has no subclassing requirements, and this class holds nothing.
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "JaynesMenuBarTarget"]
        struct Target;

        impl Target {
            #[unsafe(method(open:))]
            fn open(&self, _sender: Option<&AnyObject>) {
                CLICKED.store(true, Ordering::Relaxed);
            }
        }
    );

    thread_local! {
        static ITEM: RefCell<Option<(Retained<NSStatusItem>, Retained<Target>)>> = const { RefCell::new(None) };
        static SHOWN: RefCell<Option<usize>> = const { RefCell::new(None) };
    }

    pub fn show(waiting: usize) {
        let Some(mtm) = MainThreadMarker::new() else {
            tracing::warn!("the menu bar is drawn on the main thread only");
            return;
        };
        if SHOWN.with_borrow(|s| *s == Some(waiting)) {
            return;
        }
        SHOWN.with_borrow_mut(|s| *s = Some(waiting));
        ITEM.with_borrow_mut(|slot| {
            if slot.is_none() {
                let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
                let target: Retained<Target> = unsafe { msg_send![Target::alloc(mtm), init] };
                if let Some(button) = item.button(mtm) {
                    unsafe {
                        button.setTarget(Some(&target));
                        button.setAction(Some(sel!(open:)));
                    }
                }
                tracing::info!("menu bar item made; button: {}", item.button(mtm).is_some());
                *slot = Some((item, target));
            }
            if let Some((item, _)) = slot.as_ref()
                && let Some(button) = item.button(mtm)
            {
                let title = if waiting == 0 { "◇".to_string() } else { format!("◆ {waiting}") };
                button.setTitle(&NSString::from_str(&title));
            }
        });
        let tile = NSApplication::sharedApplication(mtm).dockTile();
        let badge = (waiting > 0).then(|| NSString::from_str(&waiting.to_string()));
        tile.setBadgeLabel(badge.as_deref());
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    pub fn show(_: usize) {}
}
