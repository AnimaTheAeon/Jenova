//! Tray icon and window visibility.
//!
//! On Windows, closing the window hides Jenova in the notification area
//! (system tray) and it keeps watching. Click the tray icon to bring the
//! window back; right-click for Open and Quit. Starting Jenova again while
//! it's in the tray just shows the window. Other platforms close normally.

use std::sync::atomic::{AtomicBool, Ordering};

/// The window is on screen (not hidden in the tray).
static WINDOW_VISIBLE: AtomicBool = AtomicBool::new(true);
/// Quit was chosen: let the window close for real.
static QUITTING: AtomicBool = AtomicBool::new(false);

pub fn window_visible() -> bool {
    WINDOW_VISIBLE.load(Ordering::Relaxed)
}

pub fn set_started_hidden() {
    WINDOW_VISIBLE.store(false, Ordering::Relaxed);
}

pub fn quitting() -> bool {
    QUITTING.load(Ordering::Relaxed)
}

/// Let the next close request end the app (updates, Quit).
pub fn allow_quit() {
    QUITTING.store(true, Ordering::Relaxed);
}

/// Whether closing the window should hide it in the tray.
pub fn available() -> bool {
    cfg!(windows)
}

#[cfg(not(windows))]
pub fn start(_ctx: eframe::egui::Context, _on_quit: Box<dyn Fn() + Send + Sync>) {}

#[cfg(not(windows))]
pub fn hide_window() {}

#[cfg(not(windows))]
#[allow(dead_code)]
pub fn show_window() {}

/// Ask a Jenova that's already running to show its window.
#[cfg(not(windows))]
pub fn signal_running_copy() -> bool {
    false
}

#[cfg(windows)]
pub use win::{hide_window, show_window, signal_running_copy, start};

#[cfg(windows)]
mod win {
    use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
    use std::sync::OnceLock;

    use windows::core::{w, BOOL, PCWSTR};
    use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, POINT, WAIT_OBJECT_0, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Threading::{
        CreateEventW, OpenEventW, SetEvent, WaitForSingleObject, EVENT_MODIFY_STATE, INFINITE,
    };
    use windows::Win32::UI::Shell::{
        Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DispatchMessageW,
        EnumWindows, GetCursorPos, GetMessageW, GetSystemMetrics, GetWindowTextW,
        GetWindowThreadProcessId, IsIconic, LoadImageW, PostMessageW, RegisterClassW,
        RegisterWindowMessageW, SetForegroundWindow, ShowWindow, TrackPopupMenu, TranslateMessage,
        HICON, IMAGE_ICON, LR_DEFAULTCOLOR, MF_DEFAULT, MF_SEPARATOR, MF_STRING, MSG,
        SM_CXSMICON, SW_HIDE, SW_RESTORE, SW_SHOW, TPM_RETURNCMD, TPM_RIGHTBUTTON, WINDOW_EX_STYLE,
        WM_APP, WM_CONTEXTMENU, WM_LBUTTONDBLCLK, WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP, WNDCLASSW,
        WS_OVERLAPPED,
    };

    use super::{QUITTING, WINDOW_VISIBLE};
    use crate::{info, warn};

    const SHOW_EVENT: PCWSTR = w!("Local\\AnimaTheAeon.Jenova.Show");
    const TRAY_MESSAGE: u32 = WM_APP + 1;
    const MENU_OPEN: usize = 1;
    const MENU_QUIT: usize = 2;
    const TITLE: &str = "Jenova";

    static MAIN_WINDOW: AtomicIsize = AtomicIsize::new(0);
    static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);
    static CTX: OnceLock<eframe::egui::Context> = OnceLock::new();
    static ON_QUIT: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

    /// Start the tray icon and listen for "show yourself" from new copies.
    pub fn start(ctx: eframe::egui::Context, on_quit: Box<dyn Fn() + Send + Sync>) {
        let _ = CTX.set(ctx);
        let _ = ON_QUIT.set(on_quit);
        std::thread::Builder::new()
            .name("tray".into())
            .spawn(run_tray)
            .ok();
        std::thread::Builder::new()
            .name("show-listener".into())
            .spawn(listen_for_show)
            .ok();
    }

    /// Ask a Jenova that's already running to show its window.
    pub fn signal_running_copy() -> bool {
        // SAFETY: plain Win32 calls with a static, NUL-terminated name.
        unsafe {
            match OpenEventW(EVENT_MODIFY_STATE, false, SHOW_EVENT) {
                Ok(event) => {
                    let ok = SetEvent(event).is_ok();
                    let _ = windows::Win32::Foundation::CloseHandle(event);
                    ok
                }
                Err(_) => false,
            }
        }
    }

    fn listen_for_show() {
        // SAFETY: plain Win32 calls; the handle lives for the whole process.
        let event: HANDLE = match unsafe { CreateEventW(None, false, false, SHOW_EVENT) } {
            Ok(event) => event,
            Err(error) => {
                warn!("Couldn't create the show event: {error}");
                return;
            }
        };
        loop {
            // SAFETY: `event` is a valid event handle.
            if unsafe { WaitForSingleObject(event, INFINITE) } == WAIT_OBJECT_0 {
                show_window();
            } else {
                return;
            }
        }
    }

    unsafe extern "system" fn find_window(hwnd: HWND, _: LPARAM) -> BOOL {
        let mut pid = 0u32;
        // SAFETY: `hwnd` comes from EnumWindows.
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        if pid != std::process::id() {
            return true.into();
        }
        let mut buffer = [0u16; 64];
        // SAFETY: `buffer` is writable for its whole length.
        let len = unsafe { GetWindowTextW(hwnd, &mut buffer) };
        if String::from_utf16_lossy(&buffer[..len.max(0) as usize]) == TITLE {
            MAIN_WINDOW.store(hwnd.0 as isize, Ordering::SeqCst);
            return false.into();
        }
        true.into()
    }

    /// Jenova's main window (found by title among this process's windows).
    fn main_window() -> Option<HWND> {
        let cached = MAIN_WINDOW.load(Ordering::SeqCst);
        if cached != 0 {
            return Some(HWND(cached as *mut _));
        }
        // SAFETY: the callback only reads window info.
        let _ = unsafe { EnumWindows(Some(find_window), LPARAM(0)) };
        let found = MAIN_WINDOW.load(Ordering::SeqCst);
        (found != 0).then(|| HWND(found as *mut _))
    }

    /// Hide the window in the tray.
    pub fn hide_window() {
        if let Some(hwnd) = main_window() {
            // SAFETY: `hwnd` is our own top-level window.
            unsafe {
                let _ = ShowWindow(hwnd, SW_HIDE);
            }
            WINDOW_VISIBLE.store(false, Ordering::Relaxed);
            info!("Hidden in the tray");
        }
    }

    /// Bring the window back to the front.
    pub fn show_window() {
        let Some(hwnd) = main_window() else {
            warn!("Couldn't find Jenova's window to show it");
            return;
        };
        WINDOW_VISIBLE.store(true, Ordering::Relaxed);
        // SAFETY: `hwnd` is our own top-level window.
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOW);
            if IsIconic(hwnd).as_bool() {
                let _ = ShowWindow(hwnd, SW_RESTORE);
            }
            let _ = SetForegroundWindow(hwnd);
        }
        if let Some(ctx) = CTX.get() {
            ctx.request_repaint();
        }
    }

    fn quit() {
        QUITTING.store(true, Ordering::Relaxed);
        info!("Quit from the tray");
        if let Some(on_quit) = ON_QUIT.get() {
            on_quit();
        }
    }

    fn notify_data(hwnd: HWND) -> NOTIFYICONDATAW {
        let mut data = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: 1,
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
            uCallbackMessage: TRAY_MESSAGE,
            ..Default::default()
        };
        for (slot, unit) in data.szTip.iter_mut().zip("Jenova".encode_utf16()) {
            *slot = unit;
        }
        // SAFETY: loads the icon embedded in our own exe (jenova.rc).
        unsafe {
            let size = GetSystemMetrics(SM_CXSMICON);
            if let Ok(module) = GetModuleHandleW(None) {
                if let Ok(icon) =
                    LoadImageW(Some(module.into()), w!("IDI_ICON1"), IMAGE_ICON, size, size, LR_DEFAULTCOLOR)
                {
                    data.hIcon = HICON(icon.0);
                }
            }
        }
        data
    }

    fn add_icon(hwnd: HWND) {
        let data = notify_data(hwnd);
        // SAFETY: `data` is fully initialised.
        if !unsafe { Shell_NotifyIconW(NIM_ADD, &data) }.as_bool() {
            warn!("Couldn't add the tray icon");
        }
    }

    fn show_menu(hwnd: HWND) {
        // SAFETY: standard popup-menu sequence on our own window.
        unsafe {
            let Ok(menu) = CreatePopupMenu() else {
                return;
            };
            let _ = AppendMenuW(menu, MF_STRING | MF_DEFAULT, MENU_OPEN, w!("Open Jenova"));
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            let _ = AppendMenuW(menu, MF_STRING, MENU_QUIT, w!("Quit Jenova"));
            let mut point = POINT::default();
            let _ = GetCursorPos(&mut point);
            // Required so the menu closes when you click elsewhere.
            let _ = SetForegroundWindow(hwnd);
            let chosen = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_RIGHTBUTTON,
                point.x,
                point.y,
                None,
                hwnd,
                None,
            );
            let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
            let _ = DestroyMenu(menu);
            match chosen.0 as usize {
                MENU_OPEN => show_window(),
                MENU_QUIT => {
                    let data = notify_data(hwnd);
                    let _ = Shell_NotifyIconW(NIM_DELETE, &data);
                    quit();
                }
                _ => {}
            }
        }
    }

    unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if msg == TRAY_MESSAGE {
            match (lparam.0 & 0xFFFF) as u32 {
                WM_LBUTTONUP | WM_LBUTTONDBLCLK => show_window(),
                WM_RBUTTONUP | WM_CONTEXTMENU => show_menu(hwnd),
                _ => {}
            }
            return LRESULT(0);
        }
        let taskbar_created = TASKBAR_CREATED.load(Ordering::Relaxed);
        if taskbar_created != 0 && msg == taskbar_created {
            // Explorer restarted: put the icon back.
            add_icon(hwnd);
            return LRESULT(0);
        }
        // SAFETY: default handling for everything else.
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }

    fn run_tray() {
        // SAFETY: registers a window class and runs a message loop on this
        // thread; the window is never shown.
        unsafe {
            let Ok(module) = GetModuleHandleW(None) else {
                return;
            };
            let class_name = w!("JenovaTray");
            let class = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: module.into(),
                lpszClassName: class_name,
                ..Default::default()
            };
            RegisterClassW(&class);
            TASKBAR_CREATED.store(RegisterWindowMessageW(w!("TaskbarCreated")), Ordering::Relaxed);
            let hwnd = match CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                class_name,
                w!("Jenova tray"),
                WS_OVERLAPPED,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(module.into()),
                None,
            ) {
                Ok(hwnd) => hwnd,
                Err(error) => {
                    warn!("Couldn't create the tray window: {error}");
                    return;
                }
            };
            add_icon(hwnd);
            let mut message = MSG::default();
            while GetMessageW(&mut message, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }
}
