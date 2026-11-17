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

/// The notice's name in the system's center: the agent or run it is about, then when it was posted.
fn notice_id(about: &str, ms: u128) -> String {
    format!("{about}#{ms}")
}

/// The agent or run a notice the person clicked was about; None for a name the app did not give.
fn clicked_id(id: &str) -> Option<String> {
    let (about, ms) = id.rsplit_once('#')?;
    (!about.is_empty() && !ms.is_empty() && ms.bytes().all(|b| b.is_ascii_digit())).then(|| about.to_owned())
}

/// Clicks on the app's notices: the agent or run each was about, for the window to open.
fn clicks() -> &'static (async_channel::Sender<String>, async_channel::Receiver<String>) {
    static CLICKS: std::sync::OnceLock<(async_channel::Sender<String>, async_channel::Receiver<String>)> = std::sync::OnceLock::new();
    CLICKS.get_or_init(async_channel::unbounded)
}

/// The agents and runs the person opens by clicking a notice, as they click.
pub fn clicked() -> async_channel::Receiver<String> {
    clicks().1.clone()
}

/// A click on a notice about `about`, as the system's center reports one (a test's driver says it too).
pub fn click(about: String) {
    let _ = clicks().0.try_send(about);
}

/// Listens for clicks on the app's notices; call once, at start. Only an app bundle has the system's center.
pub fn listen() {
    #[cfg(target_os = "macos")]
    platform::listen();
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
    use objc2::rc::Retained;
    use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
    use objc2::{define_class, msg_send, AllocAnyThread};
    use objc2_foundation::{NSBundle, NSString};
    use objc2_user_notifications::{
        UNAuthorizationOptions, UNMutableNotificationContent, UNNotificationRequest, UNNotificationResponse, UNUserNotificationCenter,
        UNUserNotificationCenterDelegate,
    };
    use std::sync::Once;

    define_class!(
        /// Hears the system's center say the person clicked one of the app's notices.
        #[unsafe(super(NSObject))]
        #[name = "JaynesNotices"]
        struct Notices;

        unsafe impl NSObjectProtocol for Notices {}

        unsafe impl UNUserNotificationCenterDelegate for Notices {
            #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
            fn did_receive(&self, _center: &UNUserNotificationCenter, response: &UNNotificationResponse, done: &block2::DynBlock<dyn Fn()>) {
                let id = response.notification().request().identifier().to_string();
                if let Some(about) = super::clicked_id(&id) {
                    super::click(about);
                }
                done.call(());
            }
        }
    );

    pub fn listen() {
        if NSBundle::mainBundle().bundleIdentifier().is_none() {
            return;
        }
        let notices: Retained<Notices> = unsafe { msg_send![Notices::alloc(), init] };
        UNUserNotificationCenter::currentNotificationCenter().setDelegate(Some(ProtocolObject::from_ref(&*notices)));
        // the center holds its delegate weakly: this one lasts as long as the app
        std::mem::forget(notices);
    }

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
        let id = NSString::from_str(&super::notice_id(&n.run, std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)));
        let request = UNNotificationRequest::requestWithIdentifier_content_trigger(&id, &content, None);
        center.addNotificationRequest_withCompletionHandler(&request, None);
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    pub fn post(_: &crate::news::News) {}
}

#[cfg(test)]
mod tests {
    use super::{clicked_id, notice_id};

    #[test]
    fn a_clicked_notice_names_the_agent_or_run_it_was_about() {
        assert_eq!(clicked_id(&notice_id("t13f", 1794957658543)), Some("t13f".to_owned()));
        // run ids hold dashes of their own
        assert_eq!(clicked_id(&notice_id("20261117-ballast-identity", 7)), Some("20261117-ballast-identity".to_owned()));
        assert_eq!(clicked_id("from another app"), None);
    }
}
