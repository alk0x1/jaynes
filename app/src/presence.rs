//! The person's touch: before an answer that leads to something that cannot be undone, macOS asks for Touch ID (or
//! the person's password, where the Mac has no Touch ID), so no program can send it while the person looks away.

/// Asks macOS for the person's touch, saying why; resolves to whether they gave it.
pub fn ask(reason: &str) -> async_channel::Receiver<Result<(), String>> {
    let (tx, rx) = async_channel::bounded(1);
    if crate::unseen::on() {
        crate::unseen::refused("Touch ID");
        let _sent = tx.try_send(Err("a test app drawing pictures asks no touch".into()));
        return rx;
    }
    ask_with(reason, tx);
    rx
}

#[cfg(target_os = "macos")]
fn ask_with(reason: &str, tx: async_channel::Sender<Result<(), String>>) {
    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_foundation::{NSError, NSString};
    use objc2_local_authentication::{LAContext, LAPolicy};

    let context = unsafe { LAContext::new() };
    let reply = RcBlock::new(move |ok: Bool, error: *mut NSError| {
        let said = if ok.as_bool() {
            Ok(())
        } else if error.is_null() {
            Err("the touch was not given".to_string())
        } else {
            Err(unsafe { (*error).localizedDescription() }.to_string())
        };
        let _sent = tx.try_send(said);
    });
    unsafe {
        context.evaluatePolicy_localizedReason_reply(LAPolicy::DeviceOwnerAuthentication, &NSString::from_str(reason), &reply);
    }
}

#[cfg(not(target_os = "macos"))]
fn ask_with(_: &str, tx: async_channel::Sender<Result<(), String>>) {
    let _sent = tx.try_send(Err("only macOS asks for the person's touch yet".into()));
}
