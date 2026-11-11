//! The OpenSubtitles.com password, kept in Windows Credential Manager
//! instead of the settings file.
//!
//! Other platforms keep it in the settings file as before.

#[cfg(windows)]
const TARGET: &str = "Jenova/OpenSubtitles.com";

/// The saved password, or an empty string.
pub fn load_password() -> String {
    #[cfg(windows)]
    {
        read_windows().unwrap_or_default()
    }
    #[cfg(not(windows))]
    {
        crate::settings::Settings::load().opensubtitlescom_password
    }
}

/// Save (or, when empty, delete) the password.
pub fn save_password(password: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        if password.is_empty() {
            delete_windows();
            Ok(())
        } else {
            write_windows(password)
        }
    }
    #[cfg(not(windows))]
    {
        // Stored with the rest of the settings on these platforms.
        let _ = password;
        Ok(())
    }
}

/// Whether the settings file should carry the password itself.
pub fn stored_in_settings() -> bool {
    cfg!(not(windows))
}

#[cfg(windows)]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn write_windows(password: &str) -> Result<(), String> {
    use windows::core::PWSTR;
    use windows::Win32::Security::Credentials::{
        CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
    };
    let mut target = wide(TARGET);
    let mut user = wide("opensubtitles.com");
    let mut blob: Vec<u8> = password
        .encode_utf16()
        .flat_map(|unit| unit.to_le_bytes())
        .collect();
    let credential = CREDENTIALW {
        Type: CRED_TYPE_GENERIC,
        TargetName: PWSTR(target.as_mut_ptr()),
        UserName: PWSTR(user.as_mut_ptr()),
        CredentialBlobSize: blob.len() as u32,
        CredentialBlob: blob.as_mut_ptr(),
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        ..Default::default()
    };
    // SAFETY: every pointer refers to a buffer that outlives the call.
    unsafe { CredWriteW(&credential, 0) }.map_err(|e| format!("Couldn't save the password: {e}"))
}

#[cfg(windows)]
fn read_windows() -> Option<String> {
    use windows::core::PCWSTR;
    use windows::Win32::Security::Credentials::{CredFree, CredReadW, CREDENTIALW, CRED_TYPE_GENERIC};
    let target = wide(TARGET);
    let mut credential: *mut CREDENTIALW = std::ptr::null_mut();
    // SAFETY: CredReadW fills `credential` with a buffer we free below.
    unsafe { CredReadW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, None, &mut credential) }.ok()?;
    if credential.is_null() {
        return None;
    }
    // SAFETY: on success `credential` points to a valid CREDENTIALW.
    let password = unsafe {
        let c = &*credential;
        let bytes = std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize);
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    };
    // SAFETY: allocated by CredReadW.
    unsafe { CredFree(credential as *const _) };
    Some(password)
}

#[cfg(windows)]
fn delete_windows() {
    use windows::core::PCWSTR;
    use windows::Win32::Security::Credentials::{CredDeleteW, CRED_TYPE_GENERIC};
    let target = wide(TARGET);
    // SAFETY: `target` is a valid, NUL-terminated string.
    let _ = unsafe { CredDeleteW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, None) };
}
