//! Shared HID access through the vendored hidapi that `build.rs` compiles.
//! Drivers use only this module; none touches the FFI directly.

use std::{
    ffi::{c_char, c_int, c_ushort, c_void, CStr},
    time::{Duration, Instant},
};

pub(super) fn hid_trace(message: impl std::fmt::Display) {
    if std::env::var_os("BATTERY_HID_TRACE").is_some() {
        eprintln!("[battery-hid] {message}");
    }
}

#[repr(C)]
struct HidDeviceInfo {
    path: *const c_char,
    vendor_id: c_ushort,
    product_id: c_ushort,
    serial_number: *const u16,
    release_number: c_ushort,
    manufacturer_string: *const u16,
    product_string: *const u16,
    usage_page: c_ushort,
    usage: c_ushort,
    interface_number: c_int,
    next: *const HidDeviceInfo,
    bus_type: c_int,
}

#[link(name = "hidapi", kind = "static")]
unsafe extern "C" {
    fn hid_enumerate(vendor_id: c_ushort, product_id: c_ushort) -> *mut HidDeviceInfo;
    fn hid_free_enumeration(devs: *mut HidDeviceInfo);
    fn hid_open_path(path: *const c_char) -> *mut c_void;
    fn hid_close(device: *mut c_void);
    fn hid_write(device: *mut c_void, data: *const u8, length: usize) -> c_int;
    fn hid_read_timeout(
        device: *mut c_void,
        data: *mut u8,
        length: usize,
        milliseconds: c_int,
    ) -> c_int;
}

pub(super) struct HidHandle(*mut c_void);

impl HidHandle {
    /// Returns the number of bytes written, or a negative value on error.
    pub(super) fn write(&self, data: &[u8]) -> c_int {
        // SAFETY: valid owned handle; the buffer outlives the call.
        unsafe { hid_write(self.0, data.as_ptr(), data.len()) }
    }

    /// Returns the report length, 0 on timeout, or a negative value on error.
    pub(super) fn read_timeout(&self, buffer: &mut [u8], milliseconds: c_int) -> c_int {
        // SAFETY: valid owned handle; the buffer outlives the call.
        unsafe { hid_read_timeout(self.0, buffer.as_mut_ptr(), buffer.len(), milliseconds) }
    }
}

impl Drop for HidHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: hid_open_path returned this handle and this is its sole owner.
            unsafe { hid_close(self.0) };
        }
    }
}

#[derive(Debug, Clone)]
pub struct HidInterface {
    pub path: String,
    pub product: String,
    pub usage_page: u16,
    pub usage: u16,
}

pub(super) fn enumerate(vid: u16, pid: u16) -> Vec<HidInterface> {
    // SAFETY: hidapi owns the linked list until hid_free_enumeration is called.
    unsafe {
        let head = hid_enumerate(vid, pid);
        let mut result = Vec::new();
        let mut current: *const HidDeviceInfo = head;
        while !current.is_null() {
            let info = &*current;
            if !info.path.is_null() {
                result.push(HidInterface {
                    path: CStr::from_ptr(info.path).to_string_lossy().into_owned(),
                    product: wide_ptr_to_string(info.product_string),
                    usage_page: info.usage_page,
                    usage: info.usage,
                });
            }
            current = info.next;
        }
        if !head.is_null() {
            hid_free_enumeration(head);
        }
        result
    }
}

fn wide_ptr_to_string(ptr: *const u16) -> String {
    if ptr.is_null() {
        return String::new();
    }
    // HIDAPI strings are NUL-terminated. Limit protects diagnostics against
    // malformed descriptors without reading unbounded memory.
    let mut units = Vec::new();
    unsafe {
        for index in 0..256 {
            let unit = *ptr.add(index);
            if unit == 0 {
                break;
            }
            units.push(unit);
        }
    }
    String::from_utf16_lossy(&units)
}

pub(super) fn open_path(path: &str) -> Option<HidHandle> {
    let path = std::ffi::CString::new(path).ok()?;
    // SAFETY: CString guarantees a valid NUL-terminated HID path.
    let handle = unsafe { hid_open_path(path.as_ptr()) };
    (!handle.is_null()).then_some(HidHandle(handle))
}

/// Read reports from the first matching interface forever, reopening it after
/// the device is unplugged. `on_report(None)` means it was reopened.
pub(super) fn watch_interface(
    vid: u16,
    pid: u16,
    matches: impl Fn(&HidInterface) -> bool,
    mut on_report: impl FnMut(Option<&[u8]>),
) {
    let mut lost = false;
    loop {
        let device = enumerate(vid, pid)
            .into_iter()
            .find(|info| matches(info))
            .and_then(|info| open_path(&info.path));
        let Some(device) = device else {
            lost = true;
            std::thread::sleep(Duration::from_secs(5));
            continue;
        };
        if std::mem::take(&mut lost) {
            on_report(None);
        }
        loop {
            let mut report = [0u8; 64];
            let read = device.read_timeout(&mut report, 1000);
            if read < 0 {
                log::info!("HID device {vid:04x}:{pid:04x} stopped responding; waiting for it");
                lost = true;
                break;
            }
            if read > 0 {
                on_report(Some(&report[..read as usize]));
            }
        }
    }
}

/// Print every report the given interfaces send on their own for `duration`.
/// Sends nothing.
pub(super) fn print_reports(interfaces: Vec<(&'static str, HidInterface)>, duration: Duration) {
    let start = Instant::now();
    let threads: Vec<_> = interfaces
        .into_iter()
        .map(|(source, info)| {
            std::thread::spawn(move || {
                let label =
                    format!("{source} page={:04x} usage={:02x}", info.usage_page, info.usage);
                let Some(device) = open_path(&info.path) else {
                    println!("{label}: cannot open");
                    return;
                };
                println!("{label}: listening");
                while start.elapsed() < duration {
                    let mut report = [0u8; 64];
                    let read = device.read_timeout(&mut report, 250);
                    if read < 0 {
                        println!("{label}: read error");
                        return;
                    }
                    if read > 0 {
                        let bytes: Vec<String> =
                            report[..read as usize].iter().map(|b| format!("{b:02x}")).collect();
                        println!("{:7.2}s {label}: {}", start.elapsed().as_secs_f32(), bytes.join(" "));
                    }
                }
            })
        })
        .collect();
    for thread in threads {
        let _ = thread.join();
    }
}
