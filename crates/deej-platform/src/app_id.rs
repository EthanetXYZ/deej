//! The app's identity for Windows notifications (AppUserModelID).
//!
//! An unpackaged app's notifications only show a name and icon if its AppUserModelID is registered under
//! `HKCU\Software\Classes\AppUserModelId\<id>` (ARCHITECTURE.md R10). The installer's Start-menu shortcut would
//! also do it, but the portable ZIP has none, so deej registers itself at startup. Writing the same values again
//! is harmless.

use std::path::Path;

pub use imp::register;

#[cfg(windows)]
mod imp {
    use std::os::windows::ffi::OsStrExt;

    use windows::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey, RegCreateKeyExW,
        RegSetValueExW,
    };
    use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
    use windows::core::PCWSTR;

    use super::*;

    fn wide(s: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
        s.as_ref().encode_wide().chain(Some(0)).collect()
    }

    /// Registers `id` with a display name and optional icon (a PNG or ICO file), and sets it for this process.
    pub fn register(id: &str, display_name: &str, icon: Option<&Path>) -> std::io::Result<()> {
        let key_path = wide(format!(r"Software\Classes\AppUserModelId\{id}"));
        let mut key = HKEY::default();

        // SAFETY: valid null-terminated strings and out pointer; the key is closed below
        unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(key_path.as_ptr()),
                None,
                None,
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                None,
                &mut key,
                None,
            )
            .ok()
            .map_err(std::io::Error::other)?;
        }

        let mut values = vec![("DisplayName", display_name.to_string())];
        if let Some(icon) = icon {
            values.push(("IconUri", icon.display().to_string()));
        }

        let mut result = Ok(());
        for (name, value) in values {
            let name = wide(name);
            let data = wide(value);
            // SAFETY: REG_SZ data is the UTF-16 string including its terminator
            let bytes = unsafe { std::slice::from_raw_parts(data.as_ptr().cast::<u8>(), data.len() * 2) };
            let err = unsafe { RegSetValueExW(key, PCWSTR(name.as_ptr()), None, REG_SZ, Some(bytes)) };
            if err.is_err() {
                result = Err(std::io::Error::other(windows::core::Error::from(err.to_hresult())));
            }
        }

        // SAFETY: opened above, closed once
        unsafe {
            let _ = RegCloseKey(key);
        }

        let id = wide(id);
        // SAFETY: valid null-terminated string
        unsafe { SetCurrentProcessExplicitAppUserModelID(PCWSTR(id.as_ptr())) }.map_err(std::io::Error::other)?;
        result
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;

    pub fn register(_id: &str, _display_name: &str, _icon: Option<&Path>) -> std::io::Result<()> {
        Ok(())
    }
}
