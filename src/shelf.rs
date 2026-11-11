//! File shelf: drop files on the island to park them, drag them out to any
//! app later. The shelf keeps paths (not copies) in shelf.json.

use crate::config::Config;
use std::cell::Cell;
use std::sync::{Arc, Mutex};
use windows::core::{implement, Ref, Result, HRESULT, HSTRING};
use windows::Win32::Foundation::{DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS, HWND, LPARAM, POINTL, WPARAM};
use windows::Win32::System::Com::{IDataObject, DVASPECT_CONTENT, FORMATETC, TYMED_HGLOBAL};
use windows::Win32::System::Ole::*;
use windows::Win32::System::SystemServices::{MK_LBUTTON, MODIFIERKEYS_FLAGS};
use windows::Win32::UI::Shell::{DragQueryFileW, IShellItem, SHCreateItemFromParsingName, BHID_DataObject, HDROP};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

/// wparam 1: files are being dragged over the island; 0: they left.
pub const WM_SHELF_DRAG: u32 = WM_APP + 12;
/// Files were dropped; take them with `DropTarget::take`.
pub const WM_SHELF_DROP: u32 = WM_APP + 13;
pub const MAX_ITEMS: usize = 20;

fn path() -> std::path::PathBuf {
    Config::dir().join("shelf.json")
}

pub fn load() -> Vec<String> {
    std::fs::read_to_string(path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn save(items: &[String]) {
    let _ = std::fs::create_dir_all(Config::dir());
    if let Ok(t) = serde_json::to_string_pretty(items) {
        let _ = std::fs::write(path(), t);
    }
}

/// Adds dropped paths on top, without duplicates, keeping at most MAX_ITEMS.
pub fn add(items: &mut Vec<String>, dropped: &[String]) {
    for p in dropped.iter().rev() {
        items.retain(|x| !x.eq_ignore_ascii_case(p));
        items.insert(0, p.clone());
    }
    items.truncate(MAX_ITEMS);
}

fn hdrop_format() -> FORMATETC {
    FORMATETC { cfFormat: CF_HDROP.0, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 }
}

fn has_files(data: &IDataObject) -> bool {
    unsafe { data.QueryGetData(&hdrop_format()) }.is_ok()
}

fn files_of(data: &IDataObject) -> Vec<String> {
    let mut out = Vec::new();
    unsafe {
        let Ok(mut medium) = data.GetData(&hdrop_format()) else { return out };
        let drop = HDROP(medium.u.hGlobal.0);
        let count = DragQueryFileW(drop, u32::MAX, None);
        for i in 0..count {
            let len = DragQueryFileW(drop, i, None) as usize;
            let mut buf = vec![0u16; len + 1];
            DragQueryFileW(drop, i, Some(&mut buf));
            out.push(String::from_utf16_lossy(&buf[..len]));
        }
        ReleaseStgMedium(&mut medium);
    }
    out
}

#[implement(IDropTarget)]
pub struct DropTarget {
    hwnd: isize,
    accepts: Cell<bool>,
    dropped: Arc<Mutex<Vec<String>>>,
}

impl DropTarget {
    fn post(&self, msg: u32, w: usize) {
        unsafe {
            let _ = PostMessageW(Some(HWND(self.hwnd as *mut _)), msg, WPARAM(w), LPARAM(0));
        }
    }

    fn effect(&self, out: *mut DROPEFFECT) {
        if let Some(e) = unsafe { out.as_mut() } {
            *e = if self.accepts.get() { DROPEFFECT_COPY } else { DROPEFFECT_NONE };
        }
    }
}

impl IDropTarget_Impl for DropTarget_Impl {
    fn DragEnter(&self, data: Ref<'_, IDataObject>, _: MODIFIERKEYS_FLAGS, _: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        let ok = data.as_ref().is_some_and(has_files);
        self.accepts.set(ok);
        self.effect(effect);
        if ok {
            self.post(WM_SHELF_DRAG, 1);
        }
        Ok(())
    }

    fn DragOver(&self, _: MODIFIERKEYS_FLAGS, _: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        self.effect(effect);
        Ok(())
    }

    fn DragLeave(&self) -> Result<()> {
        if self.accepts.replace(false) {
            self.post(WM_SHELF_DRAG, 0);
        }
        Ok(())
    }

    fn Drop(&self, data: Ref<'_, IDataObject>, _: MODIFIERKEYS_FLAGS, _: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        self.effect(effect);
        if self.accepts.replace(false) {
            if let Some(d) = data.as_ref() {
                *self.dropped.lock().unwrap() = files_of(d);
                self.post(WM_SHELF_DROP, 0);
            }
        }
        Ok(())
    }
}

/// The island's registration as a drop target.
pub struct Target {
    hwnd: HWND,
    dropped: Arc<Mutex<Vec<String>>>,
}

impl Target {
    pub fn register(hwnd: HWND) -> Option<Self> {
        let dropped = Arc::new(Mutex::new(Vec::new()));
        let target: IDropTarget = DropTarget { hwnd: hwnd.0 as isize, accepts: Cell::new(false), dropped: dropped.clone() }.into();
        unsafe { RegisterDragDrop(hwnd, &target) }.ok()?;
        Some(Self { hwnd, dropped })
    }

    pub fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.dropped.lock().unwrap())
    }
}

impl Drop for Target {
    fn drop(&mut self) {
        unsafe {
            let _ = RevokeDragDrop(self.hwnd);
        }
    }
}

#[implement(IDropSource)]
struct DropSource;

impl IDropSource_Impl for DropSource_Impl {
    fn QueryContinueDrag(&self, escape: windows::core::BOOL, keys: MODIFIERKEYS_FLAGS) -> HRESULT {
        if escape.as_bool() {
            DRAGDROP_S_CANCEL
        } else if keys.0 & MK_LBUTTON.0 == 0 {
            DRAGDROP_S_DROP
        } else {
            HRESULT(0)
        }
    }

    fn GiveFeedback(&self, _: DROPEFFECT) -> HRESULT {
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}

/// Drags a shelf file into another app (Explorer, a browser, a chat...).
/// Blocks in OLE's drag loop until the user drops or cancels.
pub fn drag_out(path: &str) -> bool {
    unsafe {
        let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(path), None) else { return false };
        let Ok(data) = item.BindToHandler::<_, IDataObject>(None, &BHID_DataObject) else { return false };
        let source: IDropSource = DropSource.into();
        let mut effect = DROPEFFECT_NONE;
        DoDragDrop(&data, &source, DROPEFFECT_COPY | DROPEFFECT_MOVE | DROPEFFECT_LINK, &mut effect) == DRAGDROP_S_DROP
    }
}

#[cfg(test)]
mod tests {
    use super::add;

    #[test]
    fn newest_on_top_without_duplicates() {
        let mut items = vec!["C:\\a.txt".to_string(), "C:\\b.txt".to_string()];
        add(&mut items, &["C:\\c.txt".into(), "C:\\A.TXT".into()]);
        assert_eq!(items, ["C:\\c.txt", "C:\\A.TXT", "C:\\b.txt"]);
        let many: Vec<String> = (0..30).map(|i| format!("C:\\{i}")).collect();
        add(&mut items, &many);
        assert_eq!(items.len(), super::MAX_ITEMS);
        assert_eq!(items[0], "C:\\0");
    }
}
