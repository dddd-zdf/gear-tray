//! Default playback device switching.
//!
//! The headset's USB dongle keeps its audio endpoint active even when the
//! headset is off, so Windows endpoint state cannot tell whether it is in use.
//! The caller decides from the HID battery reading; this module only lists
//! endpoints and changes the default.

use anyhow::{Context, Result};
use windows::{
    core::{GUID, HSTRING, PCWSTR},
    Win32::{
        Devices::FunctionDiscovery::PKEY_Device_FriendlyName,
        Media::Audio::{
            eCommunications, eConsole, eMultimedia, eRender, IMMDeviceEnumerator,
            MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
        },
        System::Com::{
            CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
            COINIT_APARTMENTTHREADED, STGM_READ,
        },
    },
};

const POLICY_CONFIG_CLIENT: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);

#[allow(non_snake_case)]
mod policy {
    use windows::core::{interface, IUnknown, IUnknown_Vtbl, HRESULT, PCWSTR};
    use windows::Win32::Media::Audio::ERole;

    /// Undocumented but long-stable interface used by the Windows Sound control
    /// panel to change the default endpoint. Only `SetDefaultEndpoint` is called;
    /// the preceding methods exist to keep the vtable layout correct.
    #[interface("f8679f50-850a-41cf-9c72-430f290290c8")]
    pub unsafe trait IPolicyConfig: IUnknown {
        fn GetMixFormat(&self, device: PCWSTR, format: *mut *mut core::ffi::c_void) -> HRESULT;
        fn GetDeviceFormat(&self, device: PCWSTR, default: i32, format: *mut *mut core::ffi::c_void) -> HRESULT;
        fn ResetDeviceFormat(&self, device: PCWSTR) -> HRESULT;
        fn SetDeviceFormat(&self, device: PCWSTR, endpoint: *mut core::ffi::c_void, mix: *mut core::ffi::c_void) -> HRESULT;
        fn GetProcessingPeriod(&self, device: PCWSTR, default: i32, period: *mut i64, minimum: *mut i64) -> HRESULT;
        fn SetProcessingPeriod(&self, device: PCWSTR, period: *mut i64) -> HRESULT;
        fn GetShareMode(&self, device: PCWSTR, mode: *mut core::ffi::c_void) -> HRESULT;
        fn SetShareMode(&self, device: PCWSTR, mode: *mut core::ffi::c_void) -> HRESULT;
        fn GetPropertyValue(&self, device: PCWSTR, key: *const core::ffi::c_void, value: *mut core::ffi::c_void) -> HRESULT;
        fn SetPropertyValue(&self, device: PCWSTR, key: *const core::ffi::c_void, value: *mut core::ffi::c_void) -> HRESULT;
        pub fn SetDefaultEndpoint(&self, device: PCWSTR, role: ERole) -> HRESULT;
        fn SetEndpointVisibility(&self, device: PCWSTR, visible: i32) -> HRESULT;
    }
}
use policy::IPolicyConfig;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub id: String,
    pub name: String,
}

impl Endpoint {
    fn matches(&self, sound_device: &str) -> bool {
        self.name.contains(sound_device)
    }
}

/// Initializes COM for the calling thread for as long as the guard lives.
struct Com(bool);
impl Com {
    fn new() -> Self {
        Com(unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok() })
    }
}
impl Drop for Com {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

/// Active playback endpoints and the current default endpoint id.
pub fn outputs() -> Result<(Vec<Endpoint>, Option<String>)> {
    let _com = Com::new();
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .context("creating audio device enumerator")?;
        let collection = enumerator
            .EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)
            .context("listing playback devices")?;
        let mut endpoints = Vec::new();
        for index in 0..collection.GetCount()? {
            let device = collection.Item(index)?;
            let id = take_string(device.GetId()?);
            let name = device
                .OpenPropertyStore(STGM_READ)
                .and_then(|store| store.GetValue(&PKEY_Device_FriendlyName))
                .map(|value| value.to_string())
                .unwrap_or_default();
            endpoints.push(Endpoint { id, name });
        }
        let current = enumerator
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .ok()
            .and_then(|device| device.GetId().ok())
            .map(|id| take_string(id));
        Ok((endpoints, current))
    }
}

unsafe fn take_string(value: windows::core::PWSTR) -> String {
    let text = unsafe { value.to_string() }.unwrap_or_default();
    unsafe { CoTaskMemFree(Some(value.0.cast())) };
    text
}

/// Makes `id` the default for every role, so calls follow the headset too.
pub fn set_default(id: &str) -> Result<()> {
    let _com = Com::new();
    let id = HSTRING::from(id);
    unsafe {
        let policy: IPolicyConfig = CoCreateInstance(&POLICY_CONFIG_CLIENT, None, CLSCTX_ALL)
            .context("creating audio policy client")?;
        for role in [eConsole, eMultimedia, eCommunications] {
            policy
                .SetDefaultEndpoint(PCWSTR(id.as_ptr()), role)
                .ok()
                .context("setting default playback device")?;
        }
    }
    Ok(())
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Decision {
    /// Endpoint to make the default, if the current one is wrong.
    pub switch_to: Option<Endpoint>,
    /// Non-headset endpoint the user was using; return to it later.
    pub remember_speaker: Option<String>,
}

/// Picks the output for a headset connection change. `headset_on` is the
/// connected headset's sound device name, and `headsets` lists the sound
/// device names of every known headset. A speaker chosen by the user is
/// remembered; without one, the first ordinary output is used. Endpoints in
/// `ignored` are never remembered or switched to.
pub fn decide(
    headset_on: Option<&str>,
    headsets: &[String],
    outputs: &[Endpoint],
    current: Option<&str>,
    saved_speaker: Option<&str>,
    ignored: &[String],
) -> Decision {
    let is_headset = |endpoint: &Endpoint| headsets.iter().any(|name| endpoint.matches(name));
    let is_speaker =
        |endpoint: &Endpoint| !is_headset(endpoint) && !ignored.contains(&endpoint.id);
    let current_endpoint = outputs.iter().find(|endpoint| Some(endpoint.id.as_str()) == current);
    let current_is_headset = current_endpoint.is_some_and(|endpoint| is_headset(endpoint));
    let remember_speaker = current_endpoint
        .filter(|endpoint| is_speaker(endpoint))
        .map(|endpoint| endpoint.id.clone());
    if let Some(headset) = headset_on {
        let switch_to = outputs
            .iter()
            .find(|endpoint| endpoint.matches(headset))
            .filter(|endpoint| Some(endpoint.id.as_str()) != current)
            .cloned();
        return Decision { switch_to, remember_speaker };
    }
    // Headset off: leave a manually chosen non-headset output alone.
    if !current_is_headset && current_endpoint.is_some() {
        return Decision { switch_to: None, remember_speaker };
    }
    let switch_to = outputs
        .iter()
        .find(|endpoint| Some(endpoint.id.as_str()) == saved_speaker && is_speaker(endpoint))
        .or_else(|| {
            outputs
                .iter()
                .find(|endpoint| is_speaker(endpoint) && !endpoint.name.contains("Hands-Free"))
        })
        .cloned();
    Decision { switch_to, remember_speaker: None }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outputs() -> Vec<Endpoint> {
        [
            ("phone", "Speakers (iPhone Hands-Free HF Audio)"),
            ("silent", "4 - VG240Y V (AMD High Definition Audio Device)"),
            ("headset", "Speakers (HyperX Cloud III S Wireless)"),
            ("monitor", "1 - LG HDR 4K (AMD High Definition Audio Device)"),
            ("desk", "Desk Speakers (Realtek(R) Audio)"),
        ]
        .into_iter()
        .map(|(id, name)| Endpoint { id: id.into(), name: name.into() })
        .collect()
    }

    fn target(decision: &Decision) -> Option<&str> {
        decision.switch_to.as_ref().map(|endpoint| endpoint.id.as_str())
    }

    const HEADSET: &str = "HyperX Cloud III S";

    fn decide(on: bool, current: &str, saved: Option<&str>) -> Decision {
        let headsets = [HEADSET.to_owned()];
        let ignored = ["silent".to_owned()];
        super::decide(on.then_some(HEADSET), &headsets, &outputs(), Some(current), saved, &ignored)
    }

    #[test]
    fn headset_on_switches_and_remembers_speaker() {
        let decision = decide(true, "desk", None);
        assert_eq!(target(&decision), Some("headset"));
        assert_eq!(decision.remember_speaker.as_deref(), Some("desk"));
        assert_eq!(target(&decide(true, "headset", None)), None);
    }

    #[test]
    fn headset_off_returns_to_saved_speaker_or_first_ordinary_output() {
        assert_eq!(target(&decide(false, "headset", Some("desk"))), Some("desk"));
        assert_eq!(target(&decide(false, "headset", Some("gone"))), Some("monitor"));
        assert_eq!(target(&decide(false, "headset", Some("headset"))), Some("monitor"));
    }

    #[test]
    fn headset_off_keeps_a_manual_choice() {
        let decision = decide(false, "desk", Some("monitor"));
        assert_eq!(target(&decision), None);
        assert_eq!(decision.remember_speaker.as_deref(), Some("desk"));
    }

    #[test]
    fn ignored_output_is_never_the_speaker() {
        // Headset turned on while another app had picked the ignored output.
        let decision = decide(true, "silent", Some("monitor"));
        assert_eq!(target(&decision), Some("headset"));
        assert_eq!(decision.remember_speaker, None);
        assert_eq!(target(&decide(false, "headset", Some("silent"))), Some("monitor"));
        // Headset off while the ignored output is in use: leave it alone.
        assert_eq!(target(&decide(false, "silent", Some("monitor"))), None);
    }
}
