use std::{
    cell::RefCell,
    path::PathBuf,
    ptr::NonNull,
    time::{SystemTime, UNIX_EPOCH},
};

use block2::RcBlock;
use mangodisk_core::{ApplicationUsagePersistence, ApplicationUsageService};
use objc2::{
    rc::Retained,
    runtime::{AnyObject, ProtocolObject},
};
use objc2_app_kit::{
    NSApplicationActivationPolicy, NSRunningApplication, NSWorkspace, NSWorkspaceApplicationKey,
    NSWorkspaceDidActivateApplicationNotification, NSWorkspaceDidDeactivateApplicationNotification,
};
use objc2_foundation::{NSNotification, NSNotificationCenter, NSObjectProtocol};

thread_local! {
    static OBSERVER: RefCell<Option<UsageObserver>> = const { RefCell::new(None) };
}

struct UsageObserver {
    center: Retained<NSNotificationCenter>,
    tokens: Vec<Retained<ProtocolObject<dyn NSObjectProtocol>>>,
    _persistence: ApplicationUsagePersistence,
}

impl Drop for UsageObserver {
    fn drop(&mut self) {
        for token in &self.tokens {
            // SAFETY: Each token is the retained observer returned by this notification center.
            unsafe {
                self.center
                    .removeObserver(&*(Retained::as_ptr(token).cast::<AnyObject>()));
            }
        }
    }
}

/// Install after Tauri is ready. Only the app's existing lifetime is observed; no extra agent runs.
pub(crate) fn install() {
    OBSERVER.with(|slot| {
        if slot.borrow().is_some() { return; }
        let persistence = match ApplicationUsageService::start_persistence() {
            Ok(persistence) => persistence,
            Err(error) => {
                log::warn!("application_usage_observer_failed stage=start_writer native_error={} outcome=system_sources_only", mangodisk_platform::diagnostics::text(&error));
                return;
            }
        };
        let workspace = NSWorkspace::sharedWorkspace();
        let center = workspace.notificationCenter();
        let mut tokens = Vec::new();
        for name in [unsafe { NSWorkspaceDidActivateApplicationNotification }, unsafe { NSWorkspaceDidDeactivateApplicationNotification }] {
            let callback = RcBlock::new(|notification: NonNull<NSNotification>| {
                // SAFETY: NSNotificationCenter supplies a live notification for this callback.
                let notification = unsafe { notification.as_ref() };
                let Some(info) = notification.userInfo() else { return; };
                let Some(value) = info.objectForKey(unsafe { NSWorkspaceApplicationKey }) else { return; };
                if let Some(application) = value.downcast_ref::<NSRunningApplication>() { observe(application); }
            });
            // SAFETY: The block captures no thread-bound state and performs no UI or file I/O.
            tokens.push(unsafe { center.addObserverForName_object_queue_usingBlock(Some(name), None, None, &callback) });
        }
        if let Some(application) = workspace.frontmostApplication() { observe(&application); }
        *slot.borrow_mut() = Some(UsageObserver { center, tokens, _persistence: persistence });
    });
}

pub(crate) fn close() {
    OBSERVER.with(|slot| {
        slot.borrow_mut().take();
    });
}

fn observe(application: &NSRunningApplication) {
    if application.activationPolicy() != NSApplicationActivationPolicy::Regular {
        return;
    }
    let (Some(identifier), Some(path)) = (
        application.bundleIdentifier(),
        application.bundleURL().and_then(|url| url.path()),
    ) else {
        return;
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis() as u64)
        .unwrap_or_default();
    ApplicationUsageService::observe(identifier.to_string(), PathBuf::from(path.to_string()), now);
}
