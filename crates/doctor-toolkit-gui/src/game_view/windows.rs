//! The game's window inside the editor's (Windows).
//!
//! The game starts in a window (`-windowed -resolution W H`, stage 32); once
//! its top-level window appears it becomes a child of the editor's window
//! (`SetParent`, `WS_CHILD` instead of the caption and frame) and is kept
//! over the game tab. Input goes to it directly, as to any window: a click
//! gives it the keyboard. When the game exits or the tab closes the window
//! is given back to the desktop.

use windows_sys::Win32::Foundation::{HWND, LPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GW_OWNER, GWL_STYLE, GetWindow, GetWindowLongPtrW, GetWindowThreadProcessId,
    IsWindow, IsWindowVisible, MoveWindow, SW_HIDE, SW_SHOW, SetForegroundWindow, SetParent,
    SetWindowLongPtrW, ShowWindow, WS_CAPTION, WS_CHILD, WS_POPUP, WS_THICKFRAME,
};

fn hwnd(handle: isize) -> HWND {
    handle as HWND
}

/// The visible top-level window of one of `pids`, if it has one.
pub fn find_window(pids: &[u32]) -> Option<isize> {
    struct Search<'a> {
        pids: &'a [u32],
        found: Option<isize>,
    }
    unsafe extern "system" fn each(window: HWND, data: LPARAM) -> windows_sys::core::BOOL {
        // SAFETY: `data` is the `Search` passed to EnumWindows below, alive
        // for the whole call.
        let search = unsafe { &mut *(data as *mut Search<'_>) };
        let mut pid = 0u32;
        // SAFETY: plain queries of a window handle given by EnumWindows.
        let visible = unsafe {
            GetWindowThreadProcessId(window, &mut pid);
            IsWindowVisible(window) != 0 && GetWindow(window, GW_OWNER).is_null()
        };
        if visible && search.pids.contains(&pid) {
            search.found = Some(window as isize);
            return 0;
        }
        1
    }
    let mut search = Search { pids, found: None };
    // SAFETY: the callback only reads windows and writes into `search`.
    unsafe {
        EnumWindows(Some(each), &mut search as *mut Search<'_> as LPARAM);
    }
    search.found
}

/// The game's window, put inside the editor's.
pub struct Embedded {
    window: isize,
    style: isize,
}

impl Embedded {
    /// Makes `window` a child of `parent`.
    pub fn attach(window: isize, parent: isize) -> Self {
        // SAFETY: Win32 calls on window handles; a handle that is no longer
        // valid makes them fail, nothing else.
        let style = unsafe { GetWindowLongPtrW(hwnd(window), GWL_STYLE) };
        let child =
            (style & !((WS_POPUP | WS_CAPTION | WS_THICKFRAME) as isize)) | WS_CHILD as isize;
        unsafe {
            SetWindowLongPtrW(hwnd(window), GWL_STYLE, child);
            SetParent(hwnd(window), hwnd(parent));
            ShowWindow(hwnd(window), SW_SHOW);
        }
        Self { window, style }
    }

    /// Whether the window still exists.
    pub fn alive(&self) -> bool {
        // SAFETY: a query of a handle.
        unsafe { IsWindow(hwnd(self.window)) != 0 }
    }

    /// Puts the window at `rect` (pixels of the editor's client area), or
    /// hides it while the tab is not shown.
    pub fn place(&self, rect: Option<[i32; 4]>) {
        // SAFETY: as in `attach`.
        unsafe {
            match rect {
                Some([x, y, w, h]) => {
                    MoveWindow(hwnd(self.window), x, y, w.max(1), h.max(1), 1);
                    ShowWindow(hwnd(self.window), SW_SHOW);
                }
                None => {
                    ShowWindow(hwnd(self.window), SW_HIDE);
                }
            }
        }
    }

    /// Gives the keyboard back to the editor.
    pub fn release(parent: isize) {
        // SAFETY: as in `attach`.
        unsafe {
            SetForegroundWindow(hwnd(parent));
        }
    }
}

impl Drop for Embedded {
    fn drop(&mut self) {
        if self.alive() {
            // SAFETY: as in `attach`; the window goes back to the desktop.
            unsafe {
                SetParent(hwnd(self.window), std::ptr::null_mut());
                SetWindowLongPtrW(hwnd(self.window), GWL_STYLE, self.style);
                ShowWindow(hwnd(self.window), SW_SHOW);
            }
        }
    }
}
