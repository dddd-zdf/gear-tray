//! Gear Tray keeps device access (`devices`, `worker`) separate from tray
//! presentation (`app`, `panel`, `tray_icons`).

mod app;
pub mod audio_output;
pub mod devices;
mod notify;
mod panel;
mod settings;
pub mod tray_icons;
mod worker;

use std::sync::{mpsc, LazyLock};
use windows::{
    core::PCSTR,
    Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryA},
};
use winit::window::Theme;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Starts the device worker and runs the tray. UI receives readings and can
/// request an immediate refresh, but never calls a HID reader itself.
pub fn run() -> anyhow::Result<()> {
    let (readings_tx, readings_rx) = mpsc::channel();
    let wake_tx = worker::start(readings_tx);
    app::run(readings_rx, wake_tx)
}

pub(crate) fn system_theme() -> Theme {
    type ShouldSystemUseDarkMode = unsafe extern "system" fn() -> bool;
    static SHOULD_DARK: LazyLock<Option<ShouldSystemUseDarkMode>> = LazyLock::new(|| unsafe {
        let ordinal = PCSTR::from_raw(138u16 as *const u8);
        LoadLibraryA(windows::core::s!("uxtheme.dll"))
            .ok()
            .and_then(|module| GetProcAddress(module, ordinal))
            .map(|proc| std::mem::transmute(proc))
    });
    if SHOULD_DARK
        .map(|function| unsafe { function() })
        .unwrap_or(true)
    {
        Theme::Dark
    } else {
        Theme::Light
    }
}
