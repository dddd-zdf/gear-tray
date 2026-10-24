//! Gear Tray keeps native device discovery separate from tray presentation.

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
use native_devices::{DeviceKind, DeviceReading, HeadsetEvent, MouseEvent, PowerState};
use winit::window::Theme;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// What the device worker should do next. There is no timer: the dongle and
/// the receiver push on/off and battery changes, so devices are read in full
/// only at startup, on Refresh, and after the dongle or receiver is replugged.
pub(crate) enum Wake {
    /// Read every device: startup, the panel's refresh button, or a replug.
    Refresh,
    /// The headset dongle reported the headset turning on or off.
    HeadsetLink(bool),
    /// The headset dongle reported a new battery percentage.
    HeadsetBattery(u8),
    /// The Logitech receiver reported the mouse turning on or off.
    MouseLink(bool),
    /// The mouse pushed an event (such as battery status) at this time.
    MouseChanged(Instant),
}

/// Owns background polling. UI receives readings and can request an immediate
/// refresh, but never calls a native HID reader itself.
pub fn run() -> anyhow::Result<()> {
    let (readings_tx, readings_rx) = mpsc::channel();
    let (wake_tx, wake_rx) = mpsc::channel();
    let headset_tx = wake_tx.clone();
    thread::spawn(move || {
        native_devices::watch_headset_events(|event| {
            let _ = headset_tx.send(match event {
                HeadsetEvent::Link(on) => Wake::HeadsetLink(on),
                HeadsetEvent::Battery(level) => Wake::HeadsetBattery(level),
                HeadsetEvent::Reconnected => Wake::Refresh,
            });
        })
    });
    let mouse_tx = wake_tx.clone();
    thread::spawn(move || {
        native_devices::watch_mouse_events(move |event| {
            let _ = mouse_tx.send(match event {
                MouseEvent::Link(on) => Wake::MouseLink(on),
                MouseEvent::Changed => Wake::MouseChanged(Instant::now()),
                MouseEvent::Reconnected => Wake::Refresh,
            });
        })
    });
    let _ = wake_tx.send(Wake::Refresh);
    thread::spawn(move || {
        let mut readings = Vec::new();
        let mut mouse_polled = Instant::now();
        while let Ok(wake) = wake_rx.recv() {
            match wake {
                Wake::Refresh => {
                    mouse_polled = Instant::now();
                    readings = poll_all(&readings);
                }
                Wake::HeadsetLink(on) => {
                    let headset = after_link_change(DeviceKind::Headset, on, &readings);
                    replace(&mut readings, DeviceKind::Headset, headset);
                }
                Wake::HeadsetBattery(level) => {
                    let mut headset: Vec<_> = readings
                        .iter()
                        .filter(|reading| reading.device_kind == DeviceKind::Headset)
                        .cloned()
                        .collect();
                    if headset.is_empty() {
                        headset = native_devices::poll_headset();
                    }
                    // The push carries the level, so no query is needed.
                    for reading in &mut headset {
                        reading.battery_percent = Some(level);
                        reading.power = PowerState::Unknown;
                    }
                    replace(&mut readings, DeviceKind::Headset, headset);
                }
                Wake::MouseLink(on) => {
                    mouse_polled = Instant::now();
                    let mouse = after_link_change(DeviceKind::Mouse, on, &readings);
                    replace(&mut readings, DeviceKind::Mouse, mouse);
                }
                // A poll that started after the event already reflects it; this
                // collapses the burst of events a mouse sends when it connects.
                Wake::MouseChanged(at) if at < mouse_polled => continue,
                Wake::MouseChanged(_) => {
                    mouse_polled = Instant::now();
                    let mouse = native_devices::poll_mouse();
                    replace(&mut readings, DeviceKind::Mouse, mouse);
                }
            }
            if readings_tx.send(readings.clone()).is_err() {
                break;
            }
        }
    });
    app::run(readings_rx, wake_tx)
}

fn replace(readings: &mut Vec<DeviceReading>, kind: DeviceKind, new: Vec<DeviceReading>) {
    readings.retain(|reading| reading.device_kind != kind);
    readings.extend(new);
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

/// Off needs no query. A device that just turned on can take a moment to
/// answer battery queries, so retry for up to five seconds.
fn after_link_change(kind: DeviceKind, on: bool, previous: &[DeviceReading]) -> Vec<DeviceReading> {
    let poll = match kind {
        DeviceKind::Headset => native_devices::poll_headset,
        DeviceKind::Mouse => native_devices::poll_mouse,
    };
    if !on {
        let mut offline: Vec<_> = previous
            .iter()
            .filter(|reading| reading.device_kind == kind)
            .cloned()
            .collect();
        for reading in &mut offline {
            reading.battery_percent = None;
            reading.power = PowerState::Offline;
        }
        return offline;
    }
    let mut readings = poll();
    for _ in 0..10 {
        if connected(&readings, kind) {
            break;
        }
        thread::sleep(Duration::from_millis(500));
        readings = poll();
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
