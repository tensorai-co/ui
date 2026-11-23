//! Notifications: `UNUserNotificationCenter` when running from a bundle, with
//! click and removal support; `osascript` for unbundled `cargo run` binaries,
//! where the notification center throws.

use std::ffi::c_void;
use std::process::Command;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::app::NotificationPermission;
use crate::backend::macos_objc::{
    Id, NIL, Sel, call_block_isize, call_block_void, class, define_class, msg, ns_string,
    rust_string, with_callback_block,
};
use crate::event::Event;

/// True when running from an `.app` bundle with an identifier.
pub(crate) fn is_bundled() -> bool {
    let bundle = msg!(Id; class("NSBundle"), "mainBundle");
    if bundle.is_null() {
        return false;
    }
    let id = msg!(Id; bundle, "bundleIdentifier");
    let path = msg!(Id; bundle, "bundlePath");
    !id.is_null() && !path.is_null() && rust_string(path).ends_with(".app")
}

fn center() -> Option<Id> {
    if !is_bundled() {
        return None;
    }
    let c = msg!(Id; class("UNUserNotificationCenter"), "currentNotificationCenter");
    (!c.is_null()).then_some(c)
}

// UNNotificationPresentationOptions: Badge | Sound | List | Banner.
const PRESENT_ALL: isize = (1 << 0) | (1 << 1) | (1 << 3) | (1 << 4);
const DEFAULT_ACTION: &str = "com.apple.UNNotificationDefaultActionIdentifier";

extern "C" fn will_present(_: Id, _: Sel, _: Id, _: Id, handler: Id) {
    // Show banners even while the app is frontmost, like other apps' messages.
    call_block_isize(handler, PRESENT_ALL);
}

extern "C" fn did_receive(_: Id, _: Sel, _: Id, response: Id, handler: Id) {
    if !response.is_null() {
        let action = rust_string(msg!(Id; response, "actionIdentifier"));
        let request = msg!(Id; msg!(Id; response, "notification"), "request");
        if action == DEFAULT_ACTION && !request.is_null() {
            let id = rust_string(msg!(Id; request, "identifier"));
            super::macos_appkit::push_app_event(Event::NotificationClicked(id));
        }
    }
    call_block_void(handler);
}

/// Sets the notification center's delegate, at launch, so a click that
/// launches the app is delivered too.
pub(crate) fn install_delegate() {
    static DELEGATE: OnceLock<usize> = OnceLock::new();
    let Some(c) = center() else { return };
    let del = *DELEGATE.get_or_init(|| {
        let cls = define_class(
            "UiNotificationDelegate",
            "NSObject",
            &[
                (
                    "userNotificationCenter:willPresentNotification:withCompletionHandler:",
                    will_present as *const c_void,
                    "v@:@@@?",
                ),
                (
                    "userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:",
                    did_receive as *const c_void,
                    "v@:@@@?",
                ),
            ],
            &["UNUserNotificationCenterDelegate"],
        );
        // Kept for the process lifetime: the center holds its delegate weakly.
        msg!(Id; msg!(Id; cls, "alloc"), "init") as usize
    });
    msg!((); c, "setDelegate:", del as Id => Id);
}

fn next_id() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    format!(
        "ui-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// Shows a notification and returns its id (generated when `id` is `None`).
pub(crate) fn show(
    id: Option<String>,
    title: &str,
    subtitle: Option<&str>,
    body: &str,
    sound: bool,
) -> String {
    let id = id.unwrap_or_else(next_id);
    let Some(c) = center() else {
        osascript(title, subtitle, body);
        return id;
    };
    let content = msg!(Id; msg!(Id; class("UNMutableNotificationContent"), "alloc"), "init");
    msg!((); content, "setTitle:", ns_string(title) => Id);
    msg!((); content, "setBody:", ns_string(body) => Id);
    if let Some(sub) = subtitle {
        msg!((); content, "setSubtitle:", ns_string(sub) => Id);
    }
    if sound {
        let s = msg!(Id; class("UNNotificationSound"), "defaultSound");
        msg!((); content, "setSound:", s => Id);
    }
    let request = msg!(Id; class("UNNotificationRequest"), "requestWithIdentifier:content:trigger:",
        ns_string(&id) => Id, content => Id, NIL => Id);
    // Objects cross into the completion handler as addresses; retained until used.
    let (c_addr, req_addr) = (
        msg!(Id; c, "retain") as usize,
        msg!(Id; request, "retain") as usize,
    );
    msg!((); content, "release");
    // alert | sound | badge
    let options: usize = (1 << 2) | (1 << 1) | (1 << 0);
    with_callback_block(
        move |granted| {
            let (c, request) = (c_addr as Id, req_addr as Id);
            if granted & 0xff != 0 {
                msg!((); c, "addNotificationRequest:withCompletionHandler:", request => Id, NIL => Id);
            }
            msg!((); request, "release");
            msg!((); c, "release");
        },
        |block| {
            msg!((); c, "requestAuthorizationWithOptions:completionHandler:", options => usize, block => Id);
        },
    );
    id
}

/// Removes a shown or pending notification.
pub(crate) fn remove(id: &str) {
    let Some(c) = center() else { return };
    let ids = msg!(Id; class("NSArray"), "arrayWithObject:", ns_string(id) => Id);
    msg!((); c, "removeDeliveredNotificationsWithIdentifiers:", ids => Id);
    msg!((); c, "removePendingNotificationRequestsWithIdentifiers:", ids => Id);
}

/// Reports the permission without asking for it.
pub(crate) fn permission(f: impl FnOnce(NotificationPermission) + Send + 'static) {
    let Some(c) = center() else {
        f(NotificationPermission::Unbundled);
        return;
    };
    with_settings(c, move |status| {
        f(match status {
            1 => NotificationPermission::Denied,
            2..=4 => NotificationPermission::Authorized, // authorized, provisional, ephemeral
            _ => NotificationPermission::NotDetermined,
        })
    });
}

/// `getNotificationSettingsWithCompletionHandler:` hands over a settings object.
fn with_settings(c: Id, f: impl FnOnce(isize) + Send + 'static) {
    with_callback_block(
        move |settings| {
            let settings = settings as Id;
            let status = if settings.is_null() {
                0
            } else {
                msg!(isize; settings, "authorizationStatus")
            };
            f(status);
        },
        |block| msg!((); c, "getNotificationSettingsWithCompletionHandler:", block => Id),
    );
}

/// Asks for permission (shows the system prompt the first time).
pub(crate) fn request_permission(f: impl FnOnce(bool) + Send + 'static) {
    let Some(c) = center() else {
        f(false);
        return;
    };
    let options: usize = (1 << 2) | (1 << 1) | (1 << 0);
    with_callback_block(
        move |granted| f(granted & 0xff != 0),
        |block| {
            msg!((); c, "requestAuthorizationWithOptions:completionHandler:", options => usize, block => Id);
        },
    );
}

fn osascript(title: &str, subtitle: Option<&str>, body: &str) {
    let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let mut script = format!(
        "display notification \"{}\" with title \"{}\"",
        esc(body),
        esc(title)
    );
    if let Some(sub) = subtitle {
        script.push_str(&format!(" subtitle \"{}\"", esc(sub)));
    }
    let _ = Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(script)
        .spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unbundled_test_binaries_report_unbundled() {
        assert!(!is_bundled());
        let (tx, rx) = std::sync::mpsc::channel();
        permission(move |p| tx.send(p).unwrap());
        assert_eq!(rx.recv().unwrap(), NotificationPermission::Unbundled);
        assert_ne!(next_id(), next_id());
    }
}
