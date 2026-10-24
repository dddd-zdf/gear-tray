//! HyperX Cloud III S Wireless, through its USB dongle.

use std::time::{Duration, Instant};

use super::{
    hid::{self, HidHandle, HidInterface},
    DeviceKind, DeviceReading, Driver, PowerState, Push, PushSink,
};

const VID: u16 = 0x03f0;
const CLOUD_III_S_PID: u16 = 0x06be;

pub struct HyperX;

fn is_battery_interface(info: &HidInterface) -> bool {
    info.usage_page == 0x01c0 && info.usage == 1
}

impl Driver for HyperX {
    fn name(&self) -> &'static str {
        "hyperx"
    }

    fn poll(&self) -> Vec<DeviceReading> {
        hid::enumerate(VID, CLOUD_III_S_PID)
            .into_iter()
            .filter(is_battery_interface)
            .map(|info| {
                let battery = hid::open_path(&info.path).and_then(|device| battery(&device));
                // The dongle's USB product name, which Windows also uses for the
                // headset's sound device.
                let product = Some(info.product).filter(|name| !name.is_empty());
                DeviceReading {
                    device_kind: DeviceKind::Headset,
                    name: product.clone().unwrap_or_else(|| "HyperX headset".to_owned()),
                    stable_id: info.path,
                    battery_percent: battery,
                    // The protocol reports 0xff for an off headset but has no
                    // charging flag, so do not guess charging from a percentage.
                    power: if battery.is_some() {
                        PowerState::Unknown
                    } else {
                        PowerState::Offline
                    },
                    sound_device: product,
                }
            })
            .collect()
    }

    fn pushes(&self) -> bool {
        true
    }

    fn watch(&self, sink: PushSink) {
        hid::watch_interface(VID, CLOUD_III_S_PID, is_battery_interface, |report| {
            match report.map(parse_push) {
                Some(Some(push)) => {
                    log::info!("Headset dongle reported {push:?}");
                    sink(push);
                }
                Some(None) => {}
                None => sink(Push::Reconnected),
            }
        })
    }

    fn interfaces(&self) -> Vec<HidInterface> {
        hid::enumerate(VID, CLOUD_III_S_PID)
    }
}

/// Reports the dongle pushes on its own, observed on this PC:
/// `0d 02 03 00 0c <1 on | 0 off>` and `0d 02 03 00 01 <percent>`.
fn parse_push(report: &[u8]) -> Option<Push> {
    match report {
        [0x0d, 0x02, 0x03, 0x00, 0x0c, on, ..] => Some(Push::Link(*on != 0)),
        [0x0d, 0x02, 0x03, 0x00, 0x01, level, ..] if *level <= 100 => Some(Push::Changed),
        _ => None,
    }
}

fn battery(device: &HidHandle) -> Option<u8> {
    // Cloud III S Wireless request captured and published by auto94:
    // 0c 02 03 01 00 06; byte 6 of the response is the percentage.
    let mut request = [0u8; 52];
    request[..6].copy_from_slice(&[0x0c, 0x02, 0x03, 0x01, 0x00, 0x06]);
    if device.write(&request) < 0 {
        return None;
    }
    let deadline = Instant::now() + Duration::from_millis(1000);
    while Instant::now() < deadline {
        let remaining = deadline
            .saturating_duration_since(Instant::now())
            .as_millis()
            .max(1) as i32;
        let mut response = [0u8; 20];
        let read = device.read_timeout(&mut response, remaining);
        if read <= 0 {
            return None;
        }
        // The response echoes the six-byte battery-query header; ignore
        // unrelated input reports rather than interpreting byte six.
        if read > 6 && response[..6] == request[..6] {
            return (response[6] <= 100).then_some(response[6]);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pushes_seen_on_cloud_iii_s() {
        let mut report = [0u8; 64];
        report[..6].copy_from_slice(&[0x0d, 0x02, 0x03, 0x00, 0x0c, 0x01]);
        assert_eq!(parse_push(&report), Some(Push::Link(true)));
        report[5] = 0;
        assert_eq!(parse_push(&report), Some(Push::Link(false)));
        // Sent just before "on"; not a link state.
        report[4] = 0x0d;
        assert_eq!(parse_push(&report), None);
        let battery = [0x0d, 0x02, 0x03, 0x00, 0x01, 0x5c, 0x0f, 0xdc];
        assert_eq!(parse_push(&battery), Some(Push::Changed));
        // Our own battery query reply.
        assert_eq!(parse_push(&[0x0c, 0x02, 0x03, 0x01, 0x00, 0x06, 93]), None);
    }
}
