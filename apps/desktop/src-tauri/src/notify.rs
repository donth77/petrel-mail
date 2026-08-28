//! Desktop notifications, and the one platform that needs its own path.
//!
//! Windows and Linux go through `tauri-plugin-notification`, which is right on
//! both: a WinRT toast on one, a D-Bus message on the other.
//!
//! macOS does not, and the reason is worth writing down because every layer
//! involved reports success. The plugin calls `notify-rust`, which calls
//! `mac-notification-sys`, which speaks `NSUserNotification` — an API Apple
//! deprecated in macOS 11 and which no longer delivers anything at all on
//! macOS 26. `mac-notification-sys` returns `Ok`. The plugin discards the
//! result anyway (`let _ = notification.show()`). So the command succeeds, the
//! JavaScript resolves, and nothing whatsoever appears. Confirmed against this
//! machine: `dev.petrel.desktop` had never once been registered in Notification
//! Center, alongside 84 applications that had.
//!
//! `UserNotifications.framework` is what replaced it, and it is what this uses.
//! Authorisation is asked for on first use, the way the framework requires;
//! after that macOS remembers, and the user's answer lives in System Settings
//! where they would expect to find it.
//!
//! One thing to know before debugging a silent build: macOS refuses
//! authorisation outright — `UNErrorDomain 1`, "Notifications are not allowed
//! for this application", returned without ever showing the user a prompt —
//! for an application it has not accepted through Gatekeeper. A locally built
//! bundle is signed but not notarised, so it is refused, and so is a throwaway
//! bundle with a fresh identifier and an ad-hoc signature: measured, both of
//! them, from a temporary directory and from ~/Applications alike. The
//! notarised build in /Applications is the one macOS will grant.
//!
//! So "no notification from `./scripts/rebuild.sh`" is the expected result
//! rather than a bug to chase, and the error now says which of the two it is.

/// Posts a notification, and says what actually happened.
///
/// The point of the return value is that the caller can tell the difference
/// between "shown" and "refused" — which is exactly what the whole stack under
/// it could not do.
pub fn post(title: &str, body: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        mac::post(title, body)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (title, body);
        Err("not the native path".into())
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use block2::RcBlock;
    use objc2_foundation::{NSError, NSString};
    use objc2_user_notifications::{
        UNAuthorizationOptions, UNMutableNotificationContent, UNNotificationRequest,
        UNUserNotificationCenter,
    };
    use std::sync::mpsc;
    use std::time::Duration;

    /// An NSError as something a person can act on, and a maintainer can look
    /// up. The code is what distinguishes "you turned this off" from "macOS
    /// will not grant this build the right to ask" — and those two want very
    /// different responses from whoever reads it.
    fn describe(err: *mut NSError, fallback: &str) -> String {
        if err.is_null() {
            return fallback.to_string();
        }
        // Safety: non-null, and owned by the framework for the call's duration.
        let (domain, code, desc) = unsafe {
            (
                (*err).domain().to_string(),
                (*err).code(),
                (*err).localizedDescription().to_string(),
            )
        };
        // Code 1 has two very different causes wearing one message, and the
        // difference is what the reader should do next: turn it back on, or
        // stop expecting a local build to work at all.
        if domain == "UNErrorDomain" && code == 1 {
            return format!(
                "{desc} ({domain} {code}) — either notifications are off for Petrel in                  System Settings, or this build is not notarised, which macOS refuses                  without ever asking."
            );
        }
        format!("{desc} ({domain} {code})")
    }

    /// Whether this process is a real application bundle.
    ///
    /// `currentNotificationCenter` raises an Objective-C exception when the
    /// process has no bundle identifier — an unwind Rust cannot catch, so it
    /// takes the app with it. `cargo tauri dev` runs an unbundled binary, and
    /// a crash on opening the settings pane would be a far worse bug than the
    /// one being fixed. Asked first, therefore, and never assumed.
    fn is_bundled() -> bool {
        use objc2_foundation::NSBundle;
        let bundle = NSBundle::mainBundle();
        bundle
            .bundleIdentifier()
            .is_some_and(|id| !id.to_string().is_empty())
    }

    pub fn post(title: &str, body: &str) -> Result<(), String> {
        if !is_bundled() {
            return Err(
                "notifications need the app bundle; this is an unbundled build".to_string(),
            );
        }

        let center = UNUserNotificationCenter::currentNotificationCenter();

        // Ask once; macOS remembers the answer and does not re-prompt. The
        // handler is required by the framework, so it carries the verdict back
        // rather than being thrown away.
        let (tx, rx) = mpsc::channel::<Result<(), String>>();
        let handler = RcBlock::new(move |granted: objc2::runtime::Bool, err: *mut NSError| {
            let outcome = if granted.as_bool() {
                Ok(())
            } else {
                Err(describe(err, "notifications are turned off for Petrel"))
            };
            let _ = tx.send(outcome);
        });
        center.requestAuthorizationWithOptions_completionHandler(
            UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
            &handler,
        );
        // Bounded: on a first run this is the system prompt, and a user who
        // walks away must not wedge the command forever. A timeout here is
        // "we do not know yet", which is the truth.
        match rx.recv_timeout(Duration::from_secs(30)) {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(e),
            Err(_) => return Err("no answer from Notification Center".to_string()),
        }

        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str(title));
        content.setBody(&NSString::from_str(body));

        // A fresh identifier each time: a repeated one replaces the notification
        // already on screen, so two arrivals would show as one.
        let id = NSString::from_str(&format!(
            "petrel-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        // No trigger means deliver it now.
        let request =
            UNNotificationRequest::requestWithIdentifier_content_trigger(&id, &content, None);

        let (tx, rx) = mpsc::channel::<Option<String>>();
        let done = RcBlock::new(move |err: *mut NSError| {
            let _ = tx.send(if err.is_null() {
                None
            } else {
                Some(describe(err, "the notification was refused"))
            });
        });
        center.addNotificationRequest_withCompletionHandler(&request, Some(&done));

        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(None) => Ok(()),
            Ok(Some(e)) => Err(e),
            // Handed over; the framework simply has not called back yet.
            Err(_) => Ok(()),
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    /// The guard that keeps a fix from becoming a crash.
    ///
    /// `currentNotificationCenter` raises an Objective-C exception when the
    /// process has no bundle identifier, and an ObjC unwind through Rust
    /// aborts. A test binary is unbundled, exactly like `cargo tauri dev`, so
    /// this asserts the refusal happens before the framework is touched at all.
    #[test]
    fn an_unbundled_process_is_refused_rather_than_crashed() {
        let err = super::post("Petrel", "Notifications are working.")
            .expect_err("an unbundled binary has no notification centre");
        assert!(err.contains("bundle"), "{err}");
    }
}
