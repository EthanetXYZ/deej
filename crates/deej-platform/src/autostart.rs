//! Start with Windows: a `HKCU\…\Run` value, aware of shortcuts in the Startup folder (port of the fork's
//! `util/autostart_windows.go`, ARCHITECTURE.md D7).
//!
//! Differences from the fork, because deej Next lives beside the original deej during the switch:
//! - Its own value name (`deej-next`), so it never overwrites the original deej's `deej` entry.
//! - Only entries that start *this* executable count as "on" and are removed when turned off. Entries that start
//!   another deej are reported in [`AutostartStatus::others`] and left alone.

use std::path::{Path, PathBuf};

/// The `Run` value deej Next writes.
pub const RUN_VALUE: &str = "deej-next";

/// Passed by the autostart entry: start in the tray, without opening the window.
pub const MINIMIZED_FLAG: &str = "--minimized";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AutostartStatus {
    /// This executable starts when the user signs in.
    pub enabled: bool,
    /// ...because of a shortcut in the Startup folder (turning autostart off deletes it).
    pub via_shortcut: bool,
    /// Other deej programs that start at sign-in (registry values or Startup shortcuts), as display strings.
    pub others: Vec<String>,
}

/// One way something starts at sign-in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    Registry { name: String, command: String },
    Shortcut { path: PathBuf, target: Option<PathBuf> },
}

impl Entry {
    fn target(&self) -> Option<PathBuf> {
        match self {
            Entry::Registry { command, .. } => command_exe(command).map(PathBuf::from),
            Entry::Shortcut { target, .. } => target.clone(),
        }
    }

    fn describe(&self) -> String {
        match self {
            Entry::Registry { name, command } => format!("Registry entry \u{201c}{name}\u{201d}: {command}"),
            Entry::Shortcut { path, target } => {
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                match target {
                    Some(t) => format!("Startup shortcut \u{201c}{name}\u{201d}: {}", t.display()),
                    None => format!("Startup shortcut \u{201c}{name}\u{201d}"),
                }
            }
        }
    }

    /// Looks like a deej (by name or target), so it's worth mentioning when it isn't this one.
    fn is_deej(&self) -> bool {
        let named = match self {
            Entry::Registry { name, .. } => name.to_lowercase().starts_with("deej"),
            Entry::Shortcut { path, .. } => {
                path.file_name().is_some_and(|n| n.to_string_lossy().to_lowercase().starts_with("deej"))
            }
        };
        let targets_deej = self
            .target()
            .and_then(|t| t.file_name().map(|n| n.to_string_lossy().to_lowercase()))
            .is_some_and(|n| n.starts_with("deej"));
        named || targets_deej
    }
}

/// The executable a `Run` command line starts: the quoted part, or everything up to `.exe`.
pub fn command_exe(command: &str) -> Option<&str> {
    let command = command.trim();
    if let Some(rest) = command.strip_prefix('"') {
        return rest.split('"').next().filter(|s| !s.is_empty());
    }
    let lower = command.to_ascii_lowercase();
    match lower.find(".exe") {
        Some(end) => Some(&command[..end + 4]),
        None => command.split_whitespace().next(),
    }
}

/// The `Run` command line for `exe`.
pub fn run_command(exe: &Path) -> String {
    format!("\"{}\" {MINIMIZED_FLAG}", exe.display())
}

/// Case-insensitive path comparison, after resolving `.`/`..` and links where the file exists.
pub fn same_file(a: &Path, b: &Path) -> bool {
    let normalize =
        |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()).to_string_lossy().to_lowercase();
    normalize(a) == normalize(b)
}

/// Works out the status from the entries found.
pub fn status_from(exe: &Path, entries: &[Entry]) -> AutostartStatus {
    let mut status = AutostartStatus::default();
    for entry in entries {
        let ours = entry.target().is_some_and(|t| same_file(&t, exe));
        if ours {
            status.enabled = true;
            status.via_shortcut |= matches!(entry, Entry::Shortcut { .. });
        } else if entry.is_deej() {
            status.others.push(entry.describe());
        }
    }
    status
}

/// Whether our Run value should be re-pointed at `exe`: only when the program it names is gone (deej was moved or
/// deleted). Another copy that still exists keeps the entry, so starting a portable or test copy once doesn't take
/// over Start with Windows.
pub fn should_refresh(entries: &[Entry], exe: &Path, exists: impl Fn(&Path) -> bool) -> bool {
    entries.iter().any(|e| match e {
        Entry::Registry { name, .. } if name.eq_ignore_ascii_case(RUN_VALUE) => match e.target() {
            Some(target) => !same_file(&target, exe) && !exists(&target),
            None => true,
        },
        _ => false,
    })
}

pub use imp::{refresh, set, status};

#[cfg(windows)]
mod imp {
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};

    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, MAX_PATH, WIN32_ERROR};
    use windows::Win32::Storage::FileSystem::WIN32_FIND_DATAW;
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize, IPersistFile,
        STGM_READ,
    };
    use windows::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE, REG_SZ, REG_VALUE_TYPE, RegCloseKey, RegDeleteValueW,
        RegEnumValueW, RegOpenKeyExW, RegSetValueExW,
    };
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    use windows::core::{Interface, PCWSTR};

    use super::*;

    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

    fn wide(s: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
        s.as_ref().encode_wide().chain(Some(0)).collect()
    }

    fn check(err: WIN32_ERROR, what: &str) -> std::io::Result<()> {
        if err.is_ok() {
            Ok(())
        } else {
            Err(std::io::Error::other(format!("{what}: {}", windows::core::Error::from(err.to_hresult()))))
        }
    }

    struct Key(HKEY);
    impl Drop for Key {
        fn drop(&mut self) {
            // SAFETY: the key was opened by RegOpenKeyExW and is closed once
            unsafe {
                let _ = RegCloseKey(self.0);
            }
        }
    }

    fn open_run_key(access: windows::Win32::System::Registry::REG_SAM_FLAGS) -> std::io::Result<Key> {
        let path = wide(RUN_KEY);
        let mut key = HKEY::default();
        // SAFETY: valid null-terminated path and out pointer
        let err = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(path.as_ptr()), None, access, &mut key) };
        check(err, "open the Run registry key")?;
        Ok(Key(key))
    }

    fn registry_entries() -> Vec<Entry> {
        let Ok(key) = open_run_key(KEY_READ) else { return Vec::new() };
        let mut entries = Vec::new();

        for index in 0.. {
            let mut name = [0u16; 512];
            let mut name_len = name.len() as u32;
            let mut data = vec![0u8; 8192];
            let mut data_len = data.len() as u32;
            let mut kind = REG_VALUE_TYPE::default();

            // SAFETY: buffers and their lengths are valid for the call
            let err = unsafe {
                RegEnumValueW(
                    key.0,
                    index,
                    Some(windows::core::PWSTR(name.as_mut_ptr())),
                    &mut name_len,
                    None,
                    Some(&mut kind.0),
                    Some(data.as_mut_ptr()),
                    Some(&mut data_len),
                )
            };
            if err.is_err() {
                break; // ERROR_NO_MORE_ITEMS, or a value too big to be a deej command line
            }
            if kind != REG_SZ && kind != windows::Win32::System::Registry::REG_EXPAND_SZ {
                continue;
            }

            let units: Vec<u16> =
                data[..data_len as usize].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            let command = String::from_utf16_lossy(&units).trim_end_matches('\0').to_string();
            entries.push(Entry::Registry { name: String::from_utf16_lossy(&name[..name_len as usize]), command });
        }

        entries
    }

    fn startup_dir() -> Option<PathBuf> {
        let app_data = std::env::var_os("APPDATA")?;
        Some(PathBuf::from(app_data).join(r"Microsoft\Windows\Start Menu\Programs\Startup"))
    }

    /// The program a `.lnk` file starts, read through the shell's own link object.
    fn shortcut_target(path: &Path) -> Option<PathBuf> {
        // SAFETY: plain COM calls on objects created and dropped here; COM is initialised for this thread below
        unsafe {
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
            link.cast::<IPersistFile>().ok()?.Load(PCWSTR(wide(path).as_ptr()), STGM_READ).ok()?;

            let mut buffer = [0u16; MAX_PATH as usize * 4];
            let mut find = WIN32_FIND_DATAW::default();
            link.GetPath(&mut buffer, &mut find, 0).ok()?;
            let len = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
            (len > 0).then(|| PathBuf::from(String::from_utf16_lossy(&buffer[..len])))
        }
    }

    fn shortcut_entries() -> Vec<Entry> {
        let Some(dir) = startup_dir() else { return Vec::new() };
        let Ok(read) = std::fs::read_dir(&dir) else { return Vec::new() };
        let links: Vec<PathBuf> = read
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("lnk")))
            .collect();
        if links.is_empty() {
            return Vec::new();
        }

        // The calling thread may already be in a single-threaded apartment (Tauri's main thread is), so the
        // shell link work runs on a short-lived thread with its own apartment.
        std::thread::spawn(move || {
            // SAFETY: paired with CoUninitialize on the same thread when it succeeded
            let initialized = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
            let entries = links
                .into_iter()
                .map(|path| {
                    let target = shortcut_target(&path);
                    Entry::Shortcut { path, target }
                })
                .collect();
            if initialized {
                // SAFETY: balances the successful CoInitializeEx above
                unsafe { CoUninitialize() };
            }
            entries
        })
        .join()
        .unwrap_or_default()
    }

    fn entries() -> Vec<Entry> {
        let mut entries = registry_entries();
        entries.extend(shortcut_entries());
        entries
    }

    /// Whether `exe` starts at sign-in, and which other deej programs do.
    pub fn status(exe: &Path) -> AutostartStatus {
        status_from(exe, &entries())
    }

    /// Points an existing `deej-next` Run value at `exe` if the program it names no longer exists, in case deej was
    /// moved since autostart was turned on (the fork's `RefreshAutostart`, narrowed by [`should_refresh`]). Returns
    /// whether it changed anything.
    pub fn refresh(exe: &Path) -> std::io::Result<bool> {
        let stale = should_refresh(&registry_entries(), exe, Path::exists);
        if stale {
            set(exe, true)?;
        }
        Ok(stale)
    }

    /// Turns starting `exe` at sign-in on or off.
    ///
    /// On: does nothing if a Startup shortcut already starts it (no second launch), otherwise writes the
    /// `deej-next` Run value (also correcting one that points at an old location). Off: removes the Run value and
    /// every Startup shortcut that starts `exe`.
    pub fn set(exe: &Path, enabled: bool) -> std::io::Result<()> {
        let entries = entries();
        let ours = |e: &&Entry| e.target().is_some_and(|t| same_file(&t, exe));

        let key = open_run_key(KEY_SET_VALUE | KEY_READ)?;
        let value = wide(RUN_VALUE);

        if enabled {
            if entries.iter().filter(ours).any(|e| matches!(e, Entry::Shortcut { .. })) {
                return Ok(());
            }
            let command = wide(run_command(exe));
            // SAFETY: REG_SZ data is the UTF-16 string including its terminator
            let bytes = unsafe { std::slice::from_raw_parts(command.as_ptr().cast::<u8>(), command.len() * 2) };
            let err = unsafe { RegSetValueExW(key.0, PCWSTR(value.as_ptr()), None, REG_SZ, Some(bytes)) };
            return check(err, "write the Run registry value");
        }

        // SAFETY: valid key and null-terminated value name
        let err = unsafe { RegDeleteValueW(key.0, PCWSTR(value.as_ptr())) };
        if err != ERROR_FILE_NOT_FOUND {
            check(err, "delete the Run registry value")?;
        }
        for entry in entries.iter().filter(ours) {
            if let Entry::Shortcut { path, .. } = entry {
                std::fs::remove_file(path)?;
            }
        }
        Ok(())
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;

    pub fn status(_exe: &Path) -> AutostartStatus {
        AutostartStatus::default()
    }

    pub fn set(_exe: &Path, _enabled: bool) -> std::io::Result<()> {
        Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "autostart is only supported on Windows"))
    }

    pub fn refresh(_exe: &Path) -> std::io::Result<bool> {
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_lines() {
        assert_eq!(command_exe(r#""C:\deej next\deej-next.exe" --minimized"#), Some(r"C:\deej next\deej-next.exe"));
        assert_eq!(command_exe(r"C:\deej\deej.exe --settings"), Some(r"C:\deej\deej.exe"));
        assert_eq!(command_exe(r"C:\deej\DEEJ.EXE"), Some(r"C:\deej\DEEJ.EXE"));
        assert_eq!(command_exe(r#""""#), None);
        assert_eq!(run_command(Path::new(r"C:\x\deej-next.exe")), r#""C:\x\deej-next.exe" --minimized"#);
    }

    #[test]
    fn status_tells_this_exe_from_other_deejs() {
        let exe = Path::new(r"C:\apps\deej-next.exe");
        let entries = vec![
            Entry::Registry { name: "OneDrive".into(), command: r#""C:\od\OneDrive.exe" /background"#.into() },
            Entry::Shortcut { path: r"C:\Startup\deej.exe.lnk".into(), target: Some(r"C:\deej\deej.exe".into()) },
            Entry::Registry { name: "deej".into(), command: r#""C:\deej\deej.exe""#.into() },
        ];
        let status = status_from(exe, &entries);
        assert!(!status.enabled);
        assert_eq!(status.others.len(), 2, "{:?}", status.others);

        let mut with_ours = entries.clone();
        with_ours.push(Entry::Registry { name: RUN_VALUE.into(), command: run_command(&exe.to_ascii_uppercase()) });
        let status = status_from(exe, &with_ours);
        assert!(status.enabled && !status.via_shortcut);

        let shortcut = vec![Entry::Shortcut { path: r"C:\Startup\mine.lnk".into(), target: Some(exe.into()) }];
        let status = status_from(exe, &shortcut);
        assert!(status.enabled && status.via_shortcut && status.others.is_empty());
    }

    #[test]
    fn only_a_missing_program_hands_the_entry_over() {
        let exe = Path::new(r"C:UsersmeAppDataLocaldeejdeej-next.exe");
        let installed =
            |target: &str| vec![Entry::Registry { name: RUN_VALUE.into(), command: run_command(Path::new(target)) }];
        let other = r"D:portabledeej-next.exe";

        // another copy that still exists keeps Start with Windows (a portable or test copy started once)
        assert!(!should_refresh(&installed(other), exe, |_| true));
        // the program it named is gone: deej was moved, so the entry follows
        assert!(should_refresh(&installed(other), exe, |_| false));
        // already ours, or not set at all: nothing to do
        assert!(!should_refresh(&installed(&exe.display().to_string()), exe, |_| false));
        assert!(!should_refresh(&[], exe, |_| false));
        // the original deej's own entry is never touched
        let original = vec![Entry::Registry { name: "deej".into(), command: r#""C:deejdeej.exe""#.into() }];
        assert!(!should_refresh(&original, exe, |_| false));
    }

    trait Upper {
        fn to_ascii_uppercase(&self) -> PathBuf;
    }
    impl Upper for Path {
        fn to_ascii_uppercase(&self) -> PathBuf {
            PathBuf::from(self.to_string_lossy().to_ascii_uppercase())
        }
    }
}
