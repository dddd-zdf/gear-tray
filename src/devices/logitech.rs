//! Logitech HID++ mice through a LIGHTSPEED/Unifying receiver or a USB cable.
//! The HID++ writes are request/response queries only; none changes a setting
//! on the receiver or mouse.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use super::{
    hid::{self, hid_trace, HidHandle, HidInterface},
    DeviceKind, DeviceReading, Driver, PowerState, Push, PushSink,
};

const VID: u16 = 0x046d;
const HIDPP_SHORT_USAGE_PAGE_MASK: u16 = 0xff00;
const HIDPP_SHORT_USAGE: u16 = 0x0001;
const HIDPP_LONG_USAGE: u16 = 0x0002;
// Wireless HID++ replies are sometimes delayed while a receiver wakes its
// paired device.  Keep this bounded: a poll gets at most three attempts for a
// single command, each capped at 300 ms, with a short pause between attempts.
const HIDPP_QUERY_TIMEOUT: Duration = Duration::from_millis(300);
const HIDPP_QUERY_ATTEMPTS: u8 = 3;
const HIDPP_RETRY_BACKOFF: Duration = Duration::from_millis(150);

pub struct Logitech;

fn is_hidpp(info: &HidInterface, usage: u16) -> bool {
    info.usage_page & HIDPP_SHORT_USAGE_PAGE_MASK == HIDPP_SHORT_USAGE_PAGE_MASK
        && info.usage == usage
}

impl Driver for Logitech {
    fn name(&self) -> &'static str {
        "logitech-hidpp"
    }

    fn poll(&self) -> Vec<DeviceReading> {
        poll_receivers()
    }

    fn pushes(&self) -> bool {
        true
    }

    /// Connection notices arrive on the short HID++ interface and feature
    /// events on the long one, so both are watched.
    fn watch(&self, sink: PushSink) {
        let threads: Vec<_> = [HIDPP_SHORT_USAGE, HIDPP_LONG_USAGE]
            .into_iter()
            .map(|usage| {
                let sink = Arc::clone(&sink);
                std::thread::spawn(move || {
                    hid::watch_interface(VID, 0, |info| is_hidpp(info, usage), |report| {
                        match report.map(parse_push) {
                            Some(Some(push)) => sink(push),
                            Some(None) => {}
                            None => sink(Push::Reconnected),
                        }
                    })
                })
            })
            .collect();
        for thread in threads {
            let _ = thread.join();
        }
    }

    fn interfaces(&self) -> Vec<HidInterface> {
        hid::enumerate(VID, 0)
            .into_iter()
            .filter(|info| is_hidpp(info, HIDPP_SHORT_USAGE) || is_hidpp(info, HIDPP_LONG_USAGE))
            .collect()
    }
}

fn parse_push(report: &[u8]) -> Option<Push> {
    match report {
        // HID++ 1.0 device connection notification; bit 6 means link lost.
        [0x10, _, 0x41, _, flags, ..] => Some(Push::Link(flags & 0x40 == 0)),
        // HID++ 2.0 event, such as battery status (sent on reconnect and when
        // the level or charging changes): a real feature index with software
        // id 0. Replies to our own queries always carry a non-zero software id.
        [0x11, _, index, 0x00, ..] if !matches!(index, 0x00 | 0x8f | 0xff) => Some(Push::Changed),
        _ => None,
    }
}

struct HidppTransport {
    write: HidHandle,
    read: HidHandle,
}

fn poll_receivers() -> Vec<DeviceReading> {
    let mut result = Vec::new();
    let interfaces = hid::enumerate(VID, 0);
    for info in interfaces.iter().filter(|info| is_hidpp(info, HIDPP_SHORT_USAGE)) {
        let Some(write) = hid::open_path(&info.path) else {
            continue;
        };
        // HID++ 2.0 requests use the short endpoint, but Lightspeed receivers
        // can deliver their replies on the matching long endpoint.
        let read_path = interfaces
            .iter()
            .find(|candidate| {
                is_hidpp(candidate, HIDPP_LONG_USAGE)
                    && same_hid_container(&candidate.path, &info.path)
            })
            .map(|candidate| candidate.path.as_str())
            .unwrap_or(info.path.as_str());
        let Some(read) = hid::open_path(read_path) else {
            continue;
        };
        let handle = HidppTransport { write, read };
        // Read-only discovery: paired wireless slots and a direct USB device.
        let slots = [1, 2, 3, 4, 5, 6, 0xff];
        for slot in slots {
            if !hidpp_ping(&handle, slot) {
                continue;
            }
            if let Some(reading) = hidpp_battery_reading(&handle, slot, &info) {
                result.push(reading);
            }
        }
    }
    result
}

fn same_hid_container(left: &str, right: &str) -> bool {
    fn key(path: &str) -> Option<(String, String)> {
        let lower = path.to_ascii_lowercase();
        let mut parts = lower.split('#');
        parts.next()?;
        let model = parts.next()?;
        let instance = parts.next()?;
        Some((
            model.split("&col").next()?.to_owned(),
            instance.rsplit_once('&')?.0.to_owned(),
        ))
    }
    key(left).is_some() && key(left) == key(right)
}

fn hidpp_ping(device: &HidppTransport, slot: u8) -> bool {
    let request = [0x10, slot, 0x00, 0x1a, 0x00, 0x00, 0x55];
    hidpp_query(device, request)
        .is_some_and(|response| response[1] == slot && response[2] == 0 && response[6] == 0x55)
}

fn hidpp_battery_reading(
    device: &HidppTransport,
    slot: u8,
    info: &HidInterface,
) -> Option<DeviceReading> {
    // Direct root-feature lookup avoids enumerating a device-controlled count
    // on every poll. These are the only feature IDs this reader needs.
    let name_feature = hidpp_feature_index(device, slot, 0x0005)?;
    let (feature, feature_index) = [0x1004, 0x1000, 0x1001].into_iter().find_map(|feature| {
        hidpp_feature_index(device, slot, feature).map(|index| (feature, index))
    })?;
    let name = hidpp_name(device, slot, name_feature)
        .filter(|value| !value.is_empty())
        .map_or_else(|| "Logitech mouse".to_owned(), |name| format!("Logitech {name}"));
    // Device type 3 means mouse in the HID++ 0x0005 feature.
    if hidpp_query(device, [0x10, slot, name_feature, 0x2a, 0, 0, 0])?[4] != 3 {
        return None;
    }
    let response = hidpp_query(device, battery_request(slot, feature, feature_index))?;
    let (percent, power) = parse_hidpp_battery(feature, &response)?;
    // A receiver path + HID++ logical slot uniquely identifies currently paired
    // devices and remains stable across regular polling.
    Some(DeviceReading {
        device_kind: DeviceKind::Mouse,
        name,
        stable_id: format!("{}#hidpp-slot-{slot}", info.path),
        battery_percent: Some(percent),
        power,
        sound_device: None,
    })
}

fn hidpp_feature_index(device: &HidppTransport, slot: u8, feature: u16) -> Option<u8> {
    let [high, low] = feature.to_be_bytes();
    let index = hidpp_query(device, [0x10, slot, 0, 0x0a, high, low, 0])?[4];
    // Index zero is the root feature, never an assigned feature.
    (index != 0).then_some(index)
}

fn hidpp_name(device: &HidppTransport, slot: u8, feature_index: u8) -> Option<String> {
    let length = hidpp_query(device, [0x10, slot, feature_index, 0x0a, 0, 0, 0])?[4] as usize;
    if length == 0 || length > 96 {
        return None;
    }
    let mut bytes = Vec::with_capacity(length);
    for offset in (0..length).step_by(3) {
        let response = hidpp_query(
            device,
            [0x10, slot, feature_index, 0x1a, offset as u8, 0, 0],
        )?;
        bytes.extend_from_slice(&response[4..7]);
    }
    bytes.truncate(length);
    String::from_utf8(bytes)
        .ok()
        .map(|name| name.trim_end_matches('\0').to_owned())
}

fn battery_request(slot: u8, feature: u16, index: u8) -> [u8; 7] {
    // BATTERY_1004 uses function 1; BATTERY_1000 and BATTERY_1001 use 0.
    let function = if feature == 0x1004 { 0x1a } else { 0x0a };
    [0x10, slot, index, function, 0, 0, 0]
}

enum HidppQueryResult {
    Reply([u8; 7]),
    TimedOut,
    DeviceError,
}

fn request_for_attempt(mut request: [u8; 7], attempt: u8) -> [u8; 7] {
    // HID++ encodes the software ID in the low nibble of the function byte.
    // A fresh ID prevents a delayed reply from a timed-out attempt being
    // mistaken for the retry's reply.
    request[3] = (request[3] & 0xf0) | ((request[3] & 0x0f) + attempt);
    request
}

fn hidpp_query(device: &HidppTransport, request: [u8; 7]) -> Option<[u8; 7]> {
    for attempt in 0..HIDPP_QUERY_ATTEMPTS {
        let attempt_request = request_for_attempt(request, attempt);
        match hidpp_query_once(device, attempt_request) {
            HidppQueryResult::Reply(reply) => return Some(reply),
            // An explicit HID++ error is definitive; retries only help when a
            // wireless reply was absent or late.
            HidppQueryResult::DeviceError => return None,
            HidppQueryResult::TimedOut if attempt + 1 < HIDPP_QUERY_ATTEMPTS => {
                hid_trace(format_args!(
                    "HID++ retry {}/{}",
                    attempt + 1,
                    HIDPP_QUERY_ATTEMPTS
                ));
                std::thread::sleep(HIDPP_RETRY_BACKOFF);
            }
            HidppQueryResult::TimedOut => return None,
        }
    }
    None
}

fn hidpp_query_once(device: &HidppTransport, request: [u8; 7]) -> HidppQueryResult {
    // Interrupt reports are exactly 7 bytes on the HID++ short endpoint
    // selected by usage page/usage.
    if device.write.write(&request) != 7 {
        hid_trace(format_args!("HID++ write failed: {request:02x?}"));
        return HidppQueryResult::TimedOut;
    }
    hid_trace(format_args!("HID++ write: {request:02x?}"));
    let deadline = Instant::now() + HIDPP_QUERY_TIMEOUT;
    while Instant::now() < deadline {
        let remaining = deadline
            .saturating_duration_since(Instant::now())
            .as_millis()
            .max(1) as i32;
        for (handle, timeout) in [(&device.write, 0), (&device.read, remaining.min(10))] {
            let mut response = [0u8; 64];
            let read = handle.read_timeout(&mut response, timeout);
            if read < 0 {
                return HidppQueryResult::TimedOut;
            }
            if read == 0 {
                continue;
            }
            hid_trace(format_args!(
                "HID++ read ({read}): {:02x?}",
                &response[..read as usize]
            ));
            if read >= 6
                && response[1] == request[1]
                && matches!(response[2], 0x8f | 0xff)
                && response[3] == request[2]
                && response[4] == request[3]
            {
                return HidppQueryResult::DeviceError;
            }
            // HID++ errors use feature index 0x8f. Match the complete command
            // header while discarding unsolicited reports until the deadline.
            if let Some(reply) = matching_reply(&request, &response[..read as usize]) {
                return HidppQueryResult::Reply(reply);
            }
        }
    }
    HidppQueryResult::TimedOut
}

fn matching_reply(request: &[u8; 7], response: &[u8]) -> Option<[u8; 7]> {
    let valid_size = matches!(
        (response.first(), response.len()),
        (Some(0x10), 7) | (Some(0x11), 20)
    );
    if valid_size && response[1..4] == request[1..4] && response[2] != 0x8f {
        Some(response[..7].try_into().unwrap())
    } else {
        None
    }
}

fn parse_hidpp_battery(feature: u16, response: &[u8; 7]) -> Option<(u8, PowerState)> {
    let percent = match feature {
        0x1000 | 0x1004 => response[4],
        0x1001 => voltage_to_percent(u16::from_be_bytes([response[4], response[5]])),
        _ => return None,
    };
    if percent > 100 {
        return None;
    }
    let power = match feature {
        0x1000 => match response[6] {
            0 => PowerState::Discharging,
            1 | 2 | 4 => PowerState::Charging,
            3 => PowerState::Full,
            _ => PowerState::Unknown,
        },
        0x1004 => match response[6] {
            0 => PowerState::Discharging,
            1 | 2 => PowerState::Charging,
            3 => PowerState::Full,
            _ => PowerState::Unknown,
        },
        0x1001 => parse_battery_1001_power(response[6]),
        _ => PowerState::Unknown,
    };
    Some((percent, power))
}

fn parse_battery_1001_power(flags: u8) -> PowerState {
    if flags & 0x80 == 0 {
        return PowerState::Discharging;
    }
    match flags & 0x07 {
        0 => PowerState::Charging,
        1 => PowerState::Full,
        2 => PowerState::Unknown,
        _ => PowerState::Unknown,
    }
}

fn voltage_to_percent(mv: u16) -> u8 {
    // Same 100-point descending voltage lookup table used by LGSTrayBattery.
    const MV: [u16; 100] = [
        4186, 4156, 4143, 4133, 4122, 4113, 4103, 4094, 4086, 4075, 4067, 4059, 4051, 4043, 4035,
        4027, 4019, 4011, 4003, 3997, 3989, 3983, 3976, 3969, 3961, 3955, 3949, 3942, 3935, 3929,
        3922, 3916, 3909, 3902, 3896, 3890, 3883, 3877, 3870, 3865, 3859, 3853, 3848, 3842, 3837,
        3833, 3828, 3824, 3819, 3815, 3811, 3808, 3804, 3800, 3797, 3793, 3790, 3787, 3784, 3781,
        3778, 3775, 3772, 3770, 3767, 3764, 3762, 3759, 3757, 3754, 3751, 3748, 3744, 3741, 3737,
        3734, 3730, 3726, 3724, 3720, 3717, 3714, 3710, 3706, 3702, 3697, 3693, 3688, 3683, 3677,
        3671, 3666, 3662, 3658, 3654, 3646, 3633, 3612, 3579, 3537,
    ];
    MV.iter()
        .position(|&threshold| mv > threshold)
        .map_or(0, |index| (100 - index) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pushes_seen_on_pro_x_2_dex() {
        let off = [0x10, 0x01, 0x41, 0x11, 0x62, 0xb8, 0x40];
        let on = [0x10, 0x01, 0x41, 0x11, 0xa2, 0xb8, 0x40];
        assert_eq!(parse_push(&off), Some(Push::Link(false)));
        assert_eq!(parse_push(&on), Some(Push::Link(true)));
        let mut battery = [0u8; 20];
        battery[..6].copy_from_slice(&[0x11, 0x01, 0x06, 0x00, 0x37, 0x04]);
        assert_eq!(parse_push(&battery), Some(Push::Changed));
        // Our own battery query reply (software id 0x0a) and ping reply.
        battery[3] = 0x1a;
        assert_eq!(parse_push(&battery), None);
        let mut ping = [0u8; 20];
        ping[..7].copy_from_slice(&[0x11, 0x01, 0x00, 0x1a, 0x04, 0x02, 0x55]);
        assert_eq!(parse_push(&ping), None);
    }

    #[test]
    fn accepts_dex_long_reply_and_rejects_unrelated_or_truncated_reports() {
        let request = [0x10, 1, 6, 0x1a, 0, 0, 0];
        let mut reply = [0u8; 20];
        reply[..7].copy_from_slice(&[0x11, 1, 6, 0x1a, 79, 8, 0]);
        assert_eq!(
            matching_reply(&request, &reply),
            Some([0x11, 1, 6, 0x1a, 79, 8, 0])
        );
        assert!(matching_reply(&request, &reply[..7]).is_none());
        reply[3] = 0x0a;
        assert!(matching_reply(&request, &reply).is_none());
    }

    #[test]
    fn parses_unified_battery_statuses() {
        assert_eq!(
            parse_hidpp_battery(0x1000, &[0x10, 1, 2, 0x0a, 76, 0, 0]),
            Some((76, PowerState::Discharging))
        );
        assert_eq!(
            parse_hidpp_battery(0x1000, &[0x10, 1, 2, 0x0a, 76, 0, 1]),
            Some((76, PowerState::Charging))
        );
        assert_eq!(
            parse_hidpp_battery(0x1004, &[0x10, 1, 2, 0x1a, 100, 0, 3]),
            Some((100, PowerState::Full))
        );
    }

    #[test]
    fn parses_voltage_battery_flags_and_lookup() {
        assert_eq!(
            parse_hidpp_battery(0x1001, &[0x10, 1, 2, 0x0a, 0x0e, 0xe4, 0]),
            Some((50, PowerState::Discharging))
        );
        assert_eq!(parse_battery_1001_power(0x80), PowerState::Charging);
        assert_eq!(parse_battery_1001_power(0x81), PowerState::Full);
        assert_eq!(voltage_to_percent(4_200), 100);
        assert_eq!(voltage_to_percent(3_500), 0);
    }

    #[test]
    fn creates_correct_read_only_battery_requests() {
        assert_eq!(battery_request(1, 0x1000, 7), [0x10, 1, 7, 0x0a, 0, 0, 0]);
        assert_eq!(battery_request(2, 0x1004, 9), [0x10, 2, 9, 0x1a, 0, 0, 0]);
    }

    #[test]
    fn retries_use_distinct_software_ids_to_ignore_late_replies() {
        let request = [0x10, 1, 6, 0x1a, 0, 0, 0];
        let first = request_for_attempt(request, 0);
        let retry = request_for_attempt(request, 1);
        assert_eq!(first[3], 0x1a);
        assert_eq!(retry[3], 0x1b);
        let late_first_reply = [0x10, 1, 6, 0x1a, 77, 0, 0];
        assert!(matching_reply(&retry, &late_first_reply).is_none());
    }
}
