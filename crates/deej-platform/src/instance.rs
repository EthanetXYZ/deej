//! Telling whether deej is already running, before anything else starts (logging rotates its files at startup,
//! which a second copy must not do to the first one's log). Tauri's single-instance plugin then hands the launch
//! over to the running copy.

pub use imp::{InstanceLock, acquire};

#[cfg(windows)]
mod imp {
    use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE};
    use windows::Win32::System::Threading::CreateMutexW;
    use windows::core::PCWSTR;

    /// Held for as long as this copy runs.
    pub struct InstanceLock(HANDLE);

    // SAFETY: the handle is only closed, once, on drop
    unsafe impl Send for InstanceLock {}
    unsafe impl Sync for InstanceLock {}

    impl Drop for InstanceLock {
        fn drop(&mut self) {
            // SAFETY: created by CreateMutexW below
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    /// `None` if another copy holds `name` (per user session).
    pub fn acquire(name: &str) -> Option<InstanceLock> {
        let wide: Vec<u16> = format!(r"Local\{name}").encode_utf16().chain(Some(0)).collect();
        // SAFETY: valid null-terminated name; the handle is owned by the returned lock
        let handle = unsafe { CreateMutexW(None, false, PCWSTR(wide.as_ptr())) }.ok()?;
        // SAFETY: reads this thread's last error, set by CreateMutexW
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            // SAFETY: our own handle to the existing mutex
            unsafe {
                let _ = CloseHandle(handle);
            }
            return None;
        }
        Some(InstanceLock(handle))
    }
}

#[cfg(not(windows))]
mod imp {
    pub struct InstanceLock;

    pub fn acquire(_name: &str) -> Option<InstanceLock> {
        Some(InstanceLock)
    }
}

#[cfg(all(test, windows))]
mod tests {
    #[test]
    fn a_second_acquire_fails_until_the_first_is_released() {
        let name = format!("deej-test-{}", std::process::id());
        let first = super::acquire(&name).expect("first");
        assert!(super::acquire(&name).is_none());
        drop(first);
        assert!(super::acquire(&name).is_some());
    }
}
