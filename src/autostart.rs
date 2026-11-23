//! Start-with-Windows decisions, ported from WinTextCapture's AutoStartDecision (v1.0.2 fix), and
//! a small adapter over the HKCU Run value that snip's install.ps1 -RunAtLogon writes.
//!
//! Contract:
//! - The Run value's presence decides whether start-at-logon is on; a config flag never does.
//! - `evaluate_startup(current, exe, stored_target_exists)`: no value -> LeaveAsIs (never switch
//!   it on by itself); value points at this exe -> LeaveAsIs; points at a missing exe -> Write
//!   (repair); points at another exe that exists -> LeaveAsIs (repair, not hijack).
//! - `evaluate_user_change(enabled, current, exe)`: off -> Remove, or LeaveAsIs when no value;
//!   on -> LeaveAsIs when it already points here, else Write (also over another copy).
//! - `target_path(command)`: the exe of a Run command; a leading quote must close; full path;
//!   None when unreadable. Quoted and unquoted forms of the same path are equal; comparison
//!   ignores case.
//! - The adapter (trait RunKey) is the only code that touches the registry; tests use a fake.
//!   Registry errors come back as Err and the tray shows TipAutoStartFail; nothing is swallowed.
//! - The startup repair runs in the tray process at start. It never creates a value: creating
//!   one is the user's own act (the Settings toggle, or install.ps1 -RunAtLogon).

use std::path::{Path, PathBuf};

/// The Run value name snip's install.ps1 uses.
pub const RUN_VALUE: &str = "Laureth snip";

/// The key that holds the per-user Run values (under HKEY_CURRENT_USER).
pub const RUN_SUBKEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutoStartAction {
    LeaveAsIs,
    Write,
    Remove,
}

impl AutoStartAction {
    pub fn name(self) -> &'static str {
        match self {
            AutoStartAction::LeaveAsIs => "leave",
            AutoStartAction::Write => "write",
            AutoStartAction::Remove => "remove",
        }
    }
}

/// Startup reconciliation (WinTextCapture's EvaluateStartup). The only repair made without the user's
/// asking is a broken entry: one whose target no longer exists. An entry pointing at another copy
/// that does exist is left alone.
pub fn evaluate_startup(current: Option<&str>, exe: &Path, stored_target_exists: bool) -> AutoStartAction {
    let Some(cmd) = current.filter(|c| !c.trim().is_empty()) else {
        return AutoStartAction::LeaveAsIs; // off; turning it on stays the user's decision
    };
    if points_to(cmd, exe) {
        return AutoStartAction::LeaveAsIs;
    }
    if stored_target_exists { AutoStartAction::LeaveAsIs } else { AutoStartAction::Write }
}

/// The user's toggle (WinTextCapture's EvaluateUserChange): the user's choice wins and points at this copy.
pub fn evaluate_user_change(enabled: bool, current: Option<&str>, exe: &Path) -> AutoStartAction {
    if !enabled {
        return if current.is_none() { AutoStartAction::LeaveAsIs } else { AutoStartAction::Remove };
    }
    match current {
        Some(cmd) if points_to(cmd, exe) => AutoStartAction::LeaveAsIs,
        _ => AutoStartAction::Write,
    }
}

/// The executable a Run command points at, as a full path; None when it cannot be read as one.
pub fn target_path(command: &str) -> Option<PathBuf> {
    let mut stored = command.trim();
    if stored.is_empty() {
        return None;
    }
    if let Some(rest) = stored.strip_prefix('"') {
        let close = rest.find('"')?;
        stored = &rest[..close];
    }
    if stored.is_empty() || stored.contains('\0') {
        return None;
    }
    // Path.GetFullPath: GetFullPathNameW on Windows, no file-system access.
    std::path::absolute(stored).ok()
}

/// The command written into the Run value for `exe`: the quoted full path, as install.ps1 writes it.
pub fn command_for(exe: &Path) -> String {
    format!("\"{}\"", exe.display())
}

fn same_path(a: &Path, b: &Path) -> bool {
    // StringComparison.OrdinalIgnoreCase.
    a.as_os_str().to_string_lossy().to_uppercase() == b.as_os_str().to_string_lossy().to_uppercase()
}

fn points_to(command: &str, exe: &Path) -> bool {
    match (target_path(command), std::path::absolute(exe)) {
        (Some(stored), Ok(exe)) => same_path(&stored, &exe),
        _ => false,
    }
}

/// The HKCU Run value, behind a trait so tests never touch the user's registry.
pub trait RunKey {
    fn read(&self) -> Result<Option<String>, String>;
    fn write(&self, command: &str) -> Result<(), String>;
    fn remove(&self) -> Result<(), String>;
}

/// Carry out `action` for `exe`. Errors come back as they are.
pub fn apply(key: &dyn RunKey, action: AutoStartAction, exe: &Path) -> Result<(), String> {
    match action {
        AutoStartAction::LeaveAsIs => Ok(()),
        AutoStartAction::Write => key.write(&command_for(exe)),
        AutoStartAction::Remove => key.remove(),
    }
}

/// Whether start at logon is on: the value exists (WinTextCapture's AutoStart.IsEnabled).
pub fn is_enabled(key: &dyn RunKey) -> Result<bool, String> {
    Ok(key.read()?.is_some())
}

/// The startup repair (WinTextCapture's ReconcileStartup). `exists` answers whether a file exists.
pub fn reconcile_startup(key: &dyn RunKey, exe: &Path, exists: impl Fn(&Path) -> bool) -> Result<AutoStartAction, String> {
    let current = key.read()?;
    let stored_exists = current.as_deref().and_then(target_path).is_some_and(|p| exists(&p));
    let action = evaluate_startup(current.as_deref(), exe, stored_exists);
    apply(key, action, exe)?;
    Ok(action)
}

/// The user's tray toggle (WinTextCapture's ApplyUserChange).
pub fn apply_user_change(key: &dyn RunKey, enabled: bool, exe: &Path) -> Result<AutoStartAction, String> {
    let current = key.read()?;
    let action = evaluate_user_change(enabled, current.as_deref(), exe);
    apply(key, action, exe)?;
    Ok(action)
}

/// The real adapter: HKCU\Software\Microsoft\Windows\CurrentVersion\Run, value `name`.
/// Used only by the tray process (start-up repair and the user's toggle). Never by a test or selftest.
#[cfg(windows)]
pub struct HkcuRun {
    pub name: String,
}

#[cfg(windows)]
impl HkcuRun {
    pub fn snip() -> HkcuRun {
        HkcuRun { name: RUN_VALUE.to_string() }
    }
}

#[cfg(windows)]
fn wz(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

#[cfg(windows)]
impl RunKey for HkcuRun {
    fn read(&self) -> Result<Option<String>, String> {
        use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_SUCCESS, ERROR_UNSUPPORTED_TYPE};
        use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};
        use windows::core::PCWSTR;
        let sub = wz(RUN_SUBKEY);
        let name = wz(&self.name);
        let mut buf: Vec<u16> = vec![0; 512];
        for _ in 0..4 {
            let mut bytes = (buf.len() * 2) as u32;
            let r = unsafe {
                RegGetValueW(HKEY_CURRENT_USER, PCWSTR(sub.as_ptr()), PCWSTR(name.as_ptr()), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as *mut _), Some(&mut bytes))
            };
            if r == ERROR_SUCCESS {
                let n = (bytes as usize / 2).min(buf.len());
                let end = buf[..n].iter().position(|&c| c == 0).unwrap_or(n);
                return Ok(Some(String::from_utf16_lossy(&buf[..end])));
            }
            if r == ERROR_FILE_NOT_FOUND {
                return Ok(None);
            }
            if r == ERROR_UNSUPPORTED_TYPE {
                // Not a string: WinTextCapture's `GetValue(..) as string` reads it as absent.
                return Ok(None);
            }
            if r == ERROR_MORE_DATA {
                buf = vec![0; bytes as usize / 2 + 2];
                continue;
            }
            return Err(format!("reading HKCU\\{RUN_SUBKEY} \"{}\": error {}", self.name, r.0));
        }
        Err(format!("reading HKCU\\{RUN_SUBKEY} \"{}\": value keeps growing", self.name))
    }

    fn write(&self, command: &str) -> Result<(), String> {
        use windows::Win32::Foundation::ERROR_SUCCESS;
        use windows::Win32::System::Registry::{HKEY_CURRENT_USER, REG_SZ, RegSetKeyValueW};
        use windows::core::PCWSTR;
        let sub = wz(RUN_SUBKEY);
        let name = wz(&self.name);
        let data = wz(command);
        let r = unsafe { RegSetKeyValueW(HKEY_CURRENT_USER, PCWSTR(sub.as_ptr()), PCWSTR(name.as_ptr()), REG_SZ.0, Some(data.as_ptr() as *const _), (data.len() * 2) as u32) };
        if r == ERROR_SUCCESS { Ok(()) } else { Err(format!("writing HKCU\\{RUN_SUBKEY} \"{}\": error {}", self.name, r.0)) }
    }

    fn remove(&self) -> Result<(), String> {
        use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
        use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RegDeleteKeyValueW};
        use windows::core::PCWSTR;
        let sub = wz(RUN_SUBKEY);
        let name = wz(&self.name);
        let r = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, PCWSTR(sub.as_ptr()), PCWSTR(name.as_ptr())) };
        // throwOnMissingValue: false
        if r == ERROR_SUCCESS || r == ERROR_FILE_NOT_FOUND { Ok(()) } else { Err(format!("removing HKCU\\{RUN_SUBKEY} \"{}\": error {}", self.name, r.0)) }
    }
}

/// A Run value in memory, for tests and the selftest. `fail` makes every call an error.
#[derive(Default)]
pub struct FakeRunKey {
    pub value: std::cell::RefCell<Option<String>>,
    pub fail: Option<String>,
    pub writes: std::cell::Cell<u32>,
    pub removes: std::cell::Cell<u32>,
}

impl FakeRunKey {
    pub fn with(value: Option<&str>) -> FakeRunKey {
        FakeRunKey { value: std::cell::RefCell::new(value.map(str::to_string)), ..Default::default() }
    }

    pub fn failing(why: &str) -> FakeRunKey {
        FakeRunKey { fail: Some(why.to_string()), ..Default::default() }
    }
}

impl RunKey for FakeRunKey {
    fn read(&self) -> Result<Option<String>, String> {
        match &self.fail {
            Some(e) => Err(e.clone()),
            None => Ok(self.value.borrow().clone()),
        }
    }

    fn write(&self, command: &str) -> Result<(), String> {
        if let Some(e) = &self.fail {
            return Err(e.clone());
        }
        self.writes.set(self.writes.get() + 1);
        *self.value.borrow_mut() = Some(command.to_string());
        Ok(())
    }

    fn remove(&self) -> Result<(), String> {
        if let Some(e) = &self.fail {
            return Err(e.clone());
        }
        self.removes.set(self.removes.get() + 1);
        *self.value.borrow_mut() = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXE: &str = r"C:\Apps\snip.exe";

    fn exe() -> &'static Path {
        Path::new(EXE)
    }

    #[test]
    fn autostart_absent_entry_left_alone() {
        assert_eq!(evaluate_startup(None, exe(), false), AutoStartAction::LeaveAsIs);
        assert_eq!(evaluate_startup(Some("   "), exe(), false), AutoStartAction::LeaveAsIs);
        // The adapter: startup never creates a value.
        let fake = FakeRunKey::with(None);
        assert_eq!(reconcile_startup(&fake, exe(), |_| false).unwrap(), AutoStartAction::LeaveAsIs);
        assert_eq!(*fake.value.borrow(), None);
        assert_eq!(fake.writes.get(), 0);
    }

    #[cfg(windows)]
    #[test]
    fn autostart_repairs_missing_executable() {
        assert_eq!(evaluate_startup(Some(r#""C:\Cleaned\snip.exe""#), exe(), false), AutoStartAction::Write);
        let fake = FakeRunKey::with(Some(r#""C:\Cleaned\snip.exe""#));
        let seen = std::cell::RefCell::new(Vec::new());
        let a = reconcile_startup(&fake, exe(), |p| {
            seen.borrow_mut().push(p.to_path_buf());
            false
        })
        .unwrap();
        assert_eq!(a, AutoStartAction::Write);
        assert_eq!(seen.borrow().as_slice(), [PathBuf::from(r"C:\Cleaned\snip.exe")]);
        assert_eq!(fake.value.borrow().as_deref(), Some(r#""C:\Apps\snip.exe""#));
        assert_eq!(fake.writes.get(), 1);
    }

    #[test]
    fn autostart_keeps_current_executable() {
        assert_eq!(evaluate_startup(Some(r#""C:\Apps\snip.exe""#), exe(), true), AutoStartAction::LeaveAsIs);
        // Pointing here wins even when the existence check says the file is gone.
        assert_eq!(evaluate_startup(Some(r#""C:\Apps\snip.exe""#), exe(), false), AutoStartAction::LeaveAsIs);
        assert_eq!(evaluate_startup(Some(r#""c:\apps\SNIP.EXE""#), exe(), false), AutoStartAction::LeaveAsIs);
    }

    #[test]
    fn autostart_does_not_hijack_other_copy() {
        assert_eq!(evaluate_startup(Some(r#""D:\Installed\snip.exe""#), exe(), true), AutoStartAction::LeaveAsIs);
        let fake = FakeRunKey::with(Some(r#""D:\Installed\snip.exe""#));
        assert_eq!(reconcile_startup(&fake, exe(), |_| true).unwrap(), AutoStartAction::LeaveAsIs);
        assert_eq!(fake.value.borrow().as_deref(), Some(r#""D:\Installed\snip.exe""#));
        assert_eq!(fake.writes.get(), 0);
    }

    #[cfg(windows)]
    #[test]
    fn autostart_quoted_and_unquoted_paths_equal() {
        assert_eq!(evaluate_startup(Some(r"C:\Apps\snip.exe"), exe(), true), AutoStartAction::LeaveAsIs);
        assert_eq!(evaluate_startup(Some(r#""C:\Apps\snip.exe""#), exe(), true), AutoStartAction::LeaveAsIs);
        assert_eq!(target_path(r#""C:\Apps\snip.exe""#), Some(PathBuf::from(r"C:\Apps\snip.exe")));
        assert_eq!(target_path(r"C:\Apps\snip.exe"), Some(PathBuf::from(r"C:\Apps\snip.exe")));
        // Arguments after the closing quote are not part of the path.
        assert_eq!(target_path(r#""C:\Apps\snip.exe" --tray"#), Some(PathBuf::from(r"C:\Apps\snip.exe")));
        // A full path: dots and forward slashes normalise like GetFullPath.
        assert_eq!(target_path(r#""C:/Apps/x/../snip.exe""#), Some(PathBuf::from(r"C:\Apps\snip.exe")));
        // Unreadable: a quote that never closes, empty, only quotes.
        assert_eq!(target_path(r#""C:\Apps\snip.exe"#), None);
        assert_eq!(target_path(""), None);
        assert_eq!(target_path(r#""""#), None);
        assert_eq!(command_for(exe()), r#""C:\Apps\snip.exe""#);
    }

    #[test]
    fn autostart_user_enable_writes() {
        assert_eq!(evaluate_user_change(true, None, exe()), AutoStartAction::Write);
        assert_eq!(evaluate_user_change(true, Some(r#""D:\Installed\snip.exe""#), exe()), AutoStartAction::Write);
        assert_eq!(evaluate_user_change(true, Some(r#""C:\Apps\snip.exe""#), exe()), AutoStartAction::LeaveAsIs);
        let fake = FakeRunKey::with(Some(r#""D:\Installed\snip.exe""#));
        assert_eq!(apply_user_change(&fake, true, exe()).unwrap(), AutoStartAction::Write);
        assert_eq!(fake.value.borrow().as_deref(), Some(r#""C:\Apps\snip.exe""#));
        assert!(is_enabled(&fake).unwrap());
    }

    #[test]
    fn autostart_user_disable_removes() {
        assert_eq!(evaluate_user_change(false, Some(r#""C:\Apps\snip.exe""#), exe()), AutoStartAction::Remove);
        assert_eq!(evaluate_user_change(false, Some(r#""D:\Other\snip.exe""#), exe()), AutoStartAction::Remove);
        assert_eq!(evaluate_user_change(false, None, exe()), AutoStartAction::LeaveAsIs);
        let fake = FakeRunKey::with(Some(r#""C:\Apps\snip.exe""#));
        assert_eq!(apply_user_change(&fake, false, exe()).unwrap(), AutoStartAction::Remove);
        assert_eq!(*fake.value.borrow(), None);
        assert_eq!(fake.removes.get(), 1);
        assert!(!is_enabled(&fake).unwrap());
    }

    #[test]
    fn autostart_adapter_reports_errors() {
        let bad = FakeRunKey::failing("access denied");
        assert_eq!(reconcile_startup(&bad, exe(), |_| false), Err("access denied".to_string()));
        assert_eq!(apply_user_change(&bad, true, exe()), Err("access denied".to_string()));
        assert_eq!(apply_user_change(&bad, false, exe()), Err("access denied".to_string()));
        assert_eq!(is_enabled(&bad), Err("access denied".to_string()));
    }
}
