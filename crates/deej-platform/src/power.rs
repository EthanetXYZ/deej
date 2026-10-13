//! Sleep and resume notifications, without a window (`PowerRegisterSuspendResumeNotification` with a callback).
//!
//! After resume, USB serial handles are usually dead and audio devices may have been re-created, so deej
//! reconnects the board and re-scans audio (ARCHITECTURE.md R16).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerEvent {
    Suspending,
    Resumed,
}

pub type PowerHandler = Box<dyn Fn(PowerEvent) + Send + Sync + 'static>;

pub use imp::PowerNotifications;

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;

    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Power::{
        DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS, HPOWERNOTIFY, PowerRegisterSuspendResumeNotification,
        PowerUnregisterSuspendResumeNotification,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        DEVICE_NOTIFY_CALLBACK, PBT_APMRESUMEAUTOMATIC, PBT_APMRESUMESUSPEND, PBT_APMSUSPEND,
    };

    use super::*;

    /// Registered for as long as it lives.
    pub struct PowerNotifications {
        registration: HPOWERNOTIFY,
        // boxed twice so the context pointer handed to Windows stays valid and thin
        _handler: Box<PowerHandler>,
        _params: Box<DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS>,
    }

    // SAFETY: the registration handle is only used to unregister, from whichever thread drops this
    unsafe impl Send for PowerNotifications {}
    unsafe impl Sync for PowerNotifications {}

    unsafe extern "system" fn callback(context: *const c_void, kind: u32, _setting: *const c_void) -> u32 {
        // SAFETY: context is the Box<PowerHandler> owned by PowerNotifications, unregistered before it's freed
        let handler = unsafe { &*(context as *const PowerHandler) };
        match kind {
            PBT_APMSUSPEND => handler(PowerEvent::Suspending),
            // AUTOMATIC arrives on every resume; RESUMESUSPEND also arrives if a user is present. deej reacts
            // to the first only, so each wake-up is handled once.
            PBT_APMRESUMEAUTOMATIC => handler(PowerEvent::Resumed),
            PBT_APMRESUMESUSPEND => {}
            _ => {}
        }
        0
    }

    impl PowerNotifications {
        pub fn register(handler: PowerHandler) -> windows::core::Result<PowerNotifications> {
            let handler = Box::new(handler);
            let mut params = Box::new(DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS {
                Callback: Some(callback),
                Context: &*handler as *const PowerHandler as *mut c_void,
            });

            let mut raw: *mut c_void = std::ptr::null_mut();
            // SAFETY: params and handler outlive the registration (both are owned by the returned value)
            let err = unsafe {
                PowerRegisterSuspendResumeNotification(
                    DEVICE_NOTIFY_CALLBACK,
                    HANDLE(&mut *params as *mut DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS as *mut c_void),
                    &mut raw,
                )
            };
            err.ok()?;
            let registration = HPOWERNOTIFY(raw as isize);

            Ok(PowerNotifications { registration, _handler: handler, _params: params })
        }
    }

    impl Drop for PowerNotifications {
        fn drop(&mut self) {
            // SAFETY: registered in `register`, unregistered once here before the handler is freed
            unsafe {
                let _ = PowerUnregisterSuspendResumeNotification(self.registration);
            }
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;

    pub struct PowerNotifications;

    impl PowerNotifications {
        pub fn register(_handler: PowerHandler) -> Result<PowerNotifications, std::io::Error> {
            Ok(PowerNotifications)
        }
    }
}
