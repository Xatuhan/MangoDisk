//! A nonactivating AppKit panel hosts the existing, Tauri-owned WebView.
use std::{
    cell::RefCell,
    sync::{atomic::Ordering, Arc},
};

use objc2::{
    define_class, msg_send, rc::Retained, DefinedClass, MainThreadMarker, MainThreadOnly, Message,
};
use objc2_app_kit::{
    NSBackingStoreType, NSPanel, NSView, NSWindow, NSWindowButton, NSWindowStyleMask,
};
use tauri::{Emitter, Manager};

struct PanelIvars {
    app: tauri::AppHandle,
}

define_class!(
    // SAFETY: NSPanel has no additional subclassing requirements. Instances and
    // their retained views never leave AppKit's main thread.
    #[unsafe(super = NSPanel)]
    #[thread_kind = MainThreadOnly]
    #[ivars = PanelIvars]
    struct ResidentPanel;

    impl ResidentPanel {
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key(&self) -> bool {
            true
        }

        #[unsafe(method(canBecomeMainWindow))]
        fn can_become_main(&self) -> bool {
            false
        }

        #[unsafe(method(becomeKeyWindow))]
        fn become_key(&self) {
            // SAFETY: The superclass method has this exact signature.
            unsafe {
                let _: () = msg_send![super(self), becomeKeyWindow];
            }
            super::focused(&self.ivars().app);
        }

        #[unsafe(method(resignKeyWindow))]
        fn resign_key(&self) {
            // SAFETY: The superclass method has this exact signature.
            unsafe {
                let _: () = msg_send![super(self), resignKeyWindow];
            }
            let app = &self.ivars().app;
            if app.state::<Arc<super::ResidentState>>().panel_open.load(Ordering::Relaxed) {
                log::info!("resident_panel_focus_changed focused=false");
                if !super::tray_owns_focus(app) {
                    super::hide(app);
                }
            }
        }
    }
);

struct Host {
    panel: Retained<ResidentPanel>,
    window: Retained<NSWindow>,
    view: Retained<NSView>,
}

thread_local! {
    static HOST: RefCell<Option<Host>> = const { RefCell::new(None) };
}

pub(super) fn create(app: &tauri::AppHandle, window: &NSWindow, view: &NSView) {
    let mtm = MainThreadMarker::new().expect("panel creation runs on the main thread");
    destroy();
    let allocated = ResidentPanel::alloc(mtm).set_ivars(PanelIvars { app: app.clone() });
    // Initialize as a real NSPanel so AppKit installs its nonactivation behavior.
    // Reclassifying Tauri's NSWindow would skip that initialization and its ivars.
    let panel: Retained<ResidentPanel> = unsafe {
        msg_send![super(allocated),
            initWithContentRect: window.frame(),
            styleMask: NSWindowStyleMask::Titled | NSWindowStyleMask::FullSizeContentView | NSWindowStyleMask::NonactivatingPanel,
            backing: NSBackingStoreType::Buffered,
            defer: false]
    };
    // SAFETY: Retained owns the panel; closing it must not release that ownership.
    unsafe {
        panel.setReleasedWhenClosed(false);
    }
    panel.setTitle(&window.title());
    panel.setTitleVisibility(objc2_app_kit::NSWindowTitleVisibility::Hidden);
    panel.setTitlebarAppearsTransparent(true);
    panel.setHidesOnDeactivate(false);
    panel.setFloatingPanel(true);
    panel.setBecomesKeyOnlyIfNeeded(false);
    panel.setLevel(window.level());
    panel.setHasShadow(true);
    panel.setCollectionBehavior(super::macos_panel_behavior(
        window.collectionBehavior(),
        objc2::available!(macos = 13.0),
    ));
    for kind in [
        NSWindowButton::CloseButton,
        NSWindowButton::MiniaturizeButton,
        NSWindowButton::ZoomButton,
    ] {
        if let Some(button) = panel.standardWindowButton(kind) {
            button.setHidden(true);
        }
    }
    // Keep Tauri's host and delegate intact; only move the retained WKWebView.
    // Its autoresizing mask follows the native panel's content bounds.
    panel.setContentView(Some(view));
    HOST.with(|host| {
        *host.borrow_mut() = Some(Host {
            panel,
            window: window.retain(),
            view: view.retain(),
        })
    });
}

fn panel() -> Option<Retained<ResidentPanel>> {
    HOST.with(|host| host.borrow().as_ref().map(|host| host.panel.clone()))
}

pub(super) fn show(app: &tauri::AppHandle) {
    let state = app.state::<Arc<super::ResidentState>>();
    if !state.panel_open.load(Ordering::Relaxed) || !state.enabled() {
        return;
    }
    let Some((panel, frame)) = HOST.with(|host| {
        host.borrow()
            .as_ref()
            .map(|host| (host.panel.clone(), host.window.frame()))
    }) else {
        return;
    };
    panel.setFrame_display(frame, true);
    // NSPanel can receive keyboard input without activating MangoDisk or moving
    // the user's fullscreen application to a different Space.
    panel.makeKeyAndOrderFront(None);
    panel.orderFrontRegardless();
    if let Some(view) = panel.contentView() {
        panel.makeFirstResponder(Some(&view));
    }
    log::info!(
        "resident_panel_native_shown active_space={} key={} visible={}",
        panel.isOnActiveSpace(),
        panel.isKeyWindow(),
        panel.isVisible()
    );
    let _ = app.emit_to(super::PANEL_LABEL, super::VISIBILITY_EVENT, true);
}

pub(super) fn hide(app: &tauri::AppHandle) {
    if app
        .state::<Arc<super::ResidentState>>()
        .panel_open
        .load(Ordering::Relaxed)
    {
        return;
    }
    if let Some(panel) = panel() {
        panel.orderOut(None);
    }
    let _ = app.emit_to(super::PANEL_LABEL, super::VISIBILITY_EVENT, false);
}

pub(super) fn is_visible() -> bool {
    panel().is_some_and(|panel| panel.isVisible())
}

pub(super) fn destroy() {
    // Release the thread-local borrow before AppKit can invoke focus callbacks.
    let Some(host) = HOST.with(|host| host.borrow_mut().take()) else {
        return;
    };
    host.panel.orderOut(None);
    host.panel.setContentView(None);
    if let Some(view) = host.window.contentView() {
        view.addSubview(&host.view);
    }
    host.panel.close();
}

pub(super) fn query(app: &tauri::AppHandle, read: fn() -> bool) -> tauri::Result<bool> {
    if MainThreadMarker::new().is_some() {
        return Ok(read());
    }
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    app.run_on_main_thread(move || {
        let _ = send.send(read());
    })?;
    receive
        .recv()
        .map_err(|_| std::io::Error::other("panel_main_thread_response_closed").into())
}
