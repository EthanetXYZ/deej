//! Process names and paths.

use std::collections::HashMap;

/// Executable name of every running process, by PID. The same source the Go deej used (go-ps reads a Toolhelp
/// snapshot), so names match it exactly, including for elevated processes that can't be opened.
#[cfg(windows)]
pub fn process_names() -> HashMap<u32, String> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
    };

    let mut names = HashMap::new();

    // SAFETY: plain Toolhelp enumeration; the snapshot handle is closed below
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return names;
        };

        let mut entry = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };

        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
                names.insert(entry.th32ProcessID, String::from_utf16_lossy(&entry.szExeFile[..len]));

                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }

        let _ = CloseHandle(snapshot);
    }

    names
}

/// Full path of a process's executable, if Windows lets us query it.
#[cfg(windows)]
pub fn exe_path(pid: u32) -> Option<String> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    };
    use windows::core::PWSTR;

    if pid == 0 {
        return None;
    }

    // SAFETY: the process handle is closed before returning; the buffer outlives the call
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;

        let mut buffer = [0u16; 1024];
        let mut size = buffer.len() as u32;
        let result = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buffer.as_mut_ptr()), &mut size);
        let _ = CloseHandle(process);

        result.ok()?;
        Some(String::from_utf16_lossy(&buffer[..size as usize]))
    }
}

#[cfg(not(windows))]
pub fn process_names() -> HashMap<u32, String> {
    HashMap::new()
}

#[cfg(not(windows))]
pub fn exe_path(_pid: u32) -> Option<String> {
    None
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn finds_this_process() {
        let pid = std::process::id();
        let names = process_names();
        let ours = names.get(&pid).expect("our own process is listed");
        assert!(ours.to_lowercase().ends_with(".exe"));

        let path = exe_path(pid).expect("our own path is readable");
        assert!(path.to_lowercase().ends_with(&ours.to_lowercase()));
    }
}
