//! Native, read-only battery readings for the gear this application supports.
//!
//! Each brand is a [`Driver`]; [`drivers`] lists them. To support new gear,
//! add a driver module and list it there. This module has no tray or UI
//! dependency, and UI code never opens HID handles.

mod hid;
mod hyperx;
mod logitech;

use std::{sync::Arc, time::Duration};

pub use hid::HidInterface;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKind {
    Headset,
    Mouse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerState {
    Charging,
    Full,
    Discharging,
    /// The receiver is present but the wireless device did not answer.
    Offline,
    /// The device answered but does not expose a documented charge state.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceReading {
    pub device_kind: DeviceKind,
    pub name: String,
    /// A HID path plus receiver slot is stable for the connected device and
    /// distinguishes multiple paired Logitech devices without exposing a UI
    /// implementation detail to callers.
    pub stable_id: String,
    pub battery_percent: Option<u8>,
    pub power: PowerState,
    /// Part of the Windows sound device name for a headset, used to switch
    /// sound to it. `None` for gear without a sound device.
    pub sound_device: Option<String>,
}

impl DeviceReading {
    pub fn is_connected(&self) -> bool {
        self.battery_percent.is_some() && self.power != PowerState::Offline
    }
}

/// Generic name for a kind, shown when no device of that kind is present.
pub fn kind_label(kind: DeviceKind) -> &'static str {
    match kind {
        DeviceKind::Headset => "Headset",
        DeviceKind::Mouse => "Mouse",
    }
}

/// The reading to show for a kind: a connected device if there is one.
pub fn primary(readings: &[DeviceReading], kind: DeviceKind) -> Option<&DeviceReading> {
    let mut of_kind = readings.iter().filter(|reading| reading.device_kind == kind);
    let first = of_kind.clone().next();
    of_kind.find(|reading| reading.is_connected()).or(first)
}

/// Something a device reported without being asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Push {
    /// The device turned on (true) or off.
    Link(bool),
    /// Battery, charging or other state changed; read the driver again.
    Changed,
    /// The dongle or receiver was plugged back in; pushes may have been missed.
    Reconnected,
}

pub type PushSink = Arc<dyn Fn(Push) + Send + Sync>;

/// One brand's protocol. Drivers only read: queries never change a device
/// setting.
pub trait Driver: Send + Sync {
    /// Short name for logs and diagnostics.
    fn name(&self) -> &'static str;
    /// Read every supported device that is present now.
    fn poll(&self) -> Vec<DeviceReading>;
    /// Whether [`Driver::watch`] reports changes. Drivers that cannot are
    /// polled on a timer instead.
    fn pushes(&self) -> bool {
        false
    }
    /// Block forever, sending what the devices report on their own.
    fn watch(&self, _sink: PushSink) {}
    /// HID interfaces this driver uses, for diagnostics.
    fn interfaces(&self) -> Vec<HidInterface>;
}

/// Every supported brand. Add new drivers here.
pub fn drivers() -> Vec<Arc<dyn Driver>> {
    vec![Arc::new(hyperx::HyperX), Arc::new(logitech::Logitech)]
}

/// Read every device from every driver.
pub fn poll_devices() -> Vec<DeviceReading> {
    drivers().iter().flat_map(|driver| driver.poll()).collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceDiagnostic {
    pub source: &'static str,
    pub stable_id: String,
    pub detail: String,
}

/// Interfaces each driver uses, without issuing any battery queries.
pub fn diagnostic_report() -> Vec<DeviceDiagnostic> {
    drivers()
        .iter()
        .flat_map(|driver| {
            driver.interfaces().into_iter().map(|info| DeviceDiagnostic {
                source: driver.name(),
                stable_id: info.path,
                detail: format!(
                    "page={:04x} usage={:02x}, product={}",
                    info.usage_page, info.usage, info.product
                ),
            })
        })
        .collect()
}

/// Print every report the drivers' interfaces send on their own for
/// `duration`. Sends nothing; used to discover push reports.
pub fn watch_reports(duration: Duration) {
    let interfaces = drivers()
        .iter()
        .flat_map(|driver| {
            let name = driver.name();
            driver.interfaces().into_iter().map(move |info| (name, info))
        })
        .collect();
    hid::print_reports(interfaces, duration);
}
