//! BatteryStatus keeps native device discovery separate from tray presentation.

mod app;
pub mod audio_output;
pub mod native_devices;
mod notify;
mod panel;
mod settings;
pub mod tray_icons;

use std::{
    sync::{mpsc, LazyLock},
    thread,
    time::{Duration, Instant},
};
use windows::{
    core::PCSTR,
    Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryA},
};
use native_devices::{DeviceKind, DeviceReading};
use winit::window::Theme;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Full poll cadence for battery levels.
const FULL_POLL: Duration = Duration::from_secs(10 * 60);
/// Retry cadence while the mouse reading is missing.
const MOUSE_RETRY: Duration = Duration::from_secs(2);

/// Why the polling worker woke up early.
pub(crate) enum Wake {
    /// The panel's refresh button.
    Refresh,
    /// The headset dongle reported the headset turning on or off.
    HeadsetLink(bool),
}

/// Owns background polling. UI receives readings and can request an immediate
/// refresh, but never calls a native HID reader itself.
pub fn run() -> anyhow::Result<()> {
    let (readings_tx, readings_rx) = mpsc::channel();
    let (wake_tx, wake_rx) = mpsc::channel();
    let link_tx = wake_tx.clone();
    thread::spawn(move || {
        native_devices::watch_headset_link(|on| {
            let _ = link_tx.send(Wake::HeadsetLink(on));
        })
    });
    thread::spawn(move || {
        let mut readings = Vec::new();
        let mut next_full = Instant::now();
        loop {
            match wake_rx.recv_timeout(next_full.saturating_duration_since(Instant::now())) {
                Ok(Wake::Refresh) | Err(mpsc::RecvTimeoutError::Timeout) => {
                    readings = poll_all(&readings);
                    let mouse_ok = connected(&readings, DeviceKind::Mouse);
                    next_full = Instant::now() + if mouse_ok { FULL_POLL } else { MOUSE_RETRY };
                }
                Ok(Wake::HeadsetLink(on)) => {
                    let headset = headset_after_link_change(on, &readings);
                    readings.retain(|reading| reading.device_kind != DeviceKind::Headset);
                    readings.splice(0..0, headset);
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
            if readings_tx.send(readings.clone()).is_err() {
                break;
            }
        }
    });
    app::run(readings_rx, wake_tx)
}

fn connected(readings: &[DeviceReading], kind: DeviceKind) -> bool {
    readings
        .iter()
        .any(|reading| reading.device_kind == kind && reading.battery_percent.is_some())
}

/// Confirm a dropped wireless reading before clearing a previously visible
/// percentage; brief receiver wake-ups can reject a query.
fn poll_all(previous: &[DeviceReading]) -> Vec<DeviceReading> {
    let readings = native_devices::poll_devices();
    let dropped = [DeviceKind::Headset, DeviceKind::Mouse]
        .into_iter()
        .any(|kind| connected(previous, kind) && !connected(&readings, kind));
    if !dropped {
        return readings;
    }
    thread::sleep(Duration::from_millis(200));
    native_devices::poll_devices()
}

/// A headset that just turned on can take a moment to answer battery queries.
fn headset_after_link_change(on: bool, previous: &[DeviceReading]) -> Vec<DeviceReading> {
    if !on {
        let mut offline: Vec<_> = previous
            .iter()
            .filter(|reading| reading.device_kind == DeviceKind::Headset)
            .cloned()
            .collect();
        if offline.is_empty() {
            return native_devices::poll_headset();
        }
        for reading in &mut offline {
            reading.battery_percent = None;
            reading.power = native_devices::PowerState::Offline;
        }
        return offline;
    }
    let mut readings = native_devices::poll_headset();
    for _ in 0..10 {
        if connected(&readings, DeviceKind::Headset) {
            break;
        }
        thread::sleep(Duration::from_millis(500));
        readings = native_devices::poll_headset();
    }
    readings
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
