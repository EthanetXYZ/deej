//! The foreground app, for the `deej.current` target.
//!
//! Ported from the Go deej (`util_windows.go`): the foreground window's process, plus the processes of its child
//! windows that belong to other processes. That covers apps hosted by another process, like UWP apps (whose top
//! window belongs to ApplicationFrameHost.exe) and launchers. Results are cached for 350 ms, as in the original,
//! because a slider sends many readings per second.

use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const CACHE_FOR: Duration = Duration::from_millis(350);

/// Remembers the last answer for [`CACHE_FOR`].
#[derive(Default)]
pub struct ForegroundApps {
    cache: Mutex<Option<(Instant, Vec<String>)>>,
}

impl ForegroundApps {
    /// Executable names of the foreground app's processes (empty if none, e.g. the desktop has focus).
    pub fn process_names(&self) -> Vec<String> {
        let mut cache = self.cache.lock().unwrap();

        if let Some((at, names)) = cache.as_ref() {
            if at.elapsed() < CACHE_FOR {
                return names.clone();
            }
        }

        let names = query();
        *cache = Some((Instant::now(), names.clone()));
        names
    }
}

#[cfg(windows)]
fn query() -> Vec<String> {
    use windows::Win32::Foundation::{HWND, LPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{EnumChildWindows, GetForegroundWindow, GetWindowThreadProcessId};
    use windows::core::BOOL;

    struct Search {
        owner: u32,
        pids: Vec<u32>,
    }

    unsafe extern "system" fn visit(child: HWND, param: LPARAM) -> BOOL {
        // SAFETY: param is the Search passed to EnumChildWindows below, alive for the whole enumeration
        let search = unsafe { &mut *(param.0 as *mut Search) };
        let mut pid = 0u32;
        // SAFETY: plain query on a window handle Windows handed us
        unsafe { GetWindowThreadProcessId(child, Some(&mut pid)) };
        if pid != 0 && pid != search.owner && !search.pids.contains(&pid) {
            search.pids.push(pid);
        }
        BOOL(1)
    }

    // SAFETY: plain window queries; the callback only touches `search`, which outlives the call
    let pids = unsafe {
        let window = GetForegroundWindow();
        let mut owner = 0u32;
        GetWindowThreadProcessId(window, Some(&mut owner));

        if owner == 0 {
            return Vec::new();
        }

        let mut search = Search { owner, pids: vec![owner] };
        let _ = EnumChildWindows(Some(window), Some(visit), LPARAM(&mut search as *mut Search as isize));
        search.pids
    };

    let names = crate::process::process_names();
    pids.iter().filter_map(|pid| names.get(pid).cloned()).collect()
}

#[cfg(not(windows))]
fn query() -> Vec<String> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_are_cached_briefly() {
        let apps = ForegroundApps::default();
        *apps.cache.lock().unwrap() = Some((Instant::now(), vec!["cached.exe".into()]));
        assert_eq!(apps.process_names(), vec!["cached.exe"]);

        *apps.cache.lock().unwrap() = Some((Instant::now() - CACHE_FOR * 2, vec!["stale.exe".into()]));
        assert_ne!(apps.process_names(), vec!["stale.exe"]);
    }
}
