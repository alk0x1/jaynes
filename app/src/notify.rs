//! Tells the person what changed while they were not looking, as a macOS notification. Only an app bundle may post
//! through the system's notification center (it names the app the notice comes from); a bare build falls back to
//! AppleScript's notice. A test keeps its notices in a file (JAYNES_APP_TEST_NOTES) and posts nothing.

use crate::news::News;

/// Posts each piece of news, unless the person is looking at the app.
pub fn post(news: &[News], looking: bool) {
    if let Some(file) = std::env::var_os("JAYNES_APP_TEST_NOTES") {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(file) {
            for n in news {
                let _kept = writeln!(f, "{}\t{}\t{}\t{}", n.run, n.title, n.body, if looking { "looking" } else { "away" });
            }
        }
        return;
    }
    if looking {
        return;
    }
    for n in news {
        platform::post(n);
    }
}

/// Whether the person is looking at the app: it is the one in front.
pub fn looking() -> bool {
    #[cfg(target_os = "macos")]
    {
        objc2::MainThreadMarker::new().is_some_and(|mtm| objc2_app_kit::NSApplication::sharedApplication(mtm).isActive())
    }
    #[cfg(not(target_os = "macos"))]
    false
}

#[cfg(target_os = "macos")]
mod platform {
    use crate::news::News;
    use objc2_foundation::{NSBundle, NSString};
    use objc2_user_notifications::{UNAuthorizationOptions, UNMutableNotificationContent, UNNotificationRequest, UNUserNotificationCenter};
    use std::sync::Once;

    pub fn post(n: &News) {
        // the system's center exists only for an app bundle: a bare build would crash asking for it
        let bundled = NSBundle::mainBundle().bundleIdentifier().is_some();
        if !bundled {
            let say = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
            let script = format!("display notification \"{}\" with title \"Jaynes\" subtitle \"{}\"", say(&n.body), say(&n.title));
            let _shown = std::process::Command::new("osascript").args(["-e", &script]).spawn();
            return;
        }
        let center = UNUserNotificationCenter::currentNotificationCenter();
        static ASKED: Once = Once::new();
        ASKED.call_once(|| {
            let done = block2::RcBlock::new(|_granted: objc2::runtime::Bool, _error: *mut objc2_foundation::NSError| {});
            center.requestAuthorizationWithOptions_completionHandler(UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound, &done);
        });
        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str(&n.title));
        content.setBody(&NSString::from_str(&n.body));
        let id = NSString::from_str(&format!("{}-{}", n.run, std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)));
        let request = UNNotificationRequest::requestWithIdentifier_content_trigger(&id, &content, None);
        center.addNotificationRequest_withCompletionHandler(&request, None);
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    pub fn post(_: &crate::news::News) {}
}
