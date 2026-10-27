use anyhow::{Context, Result};
use winreg::enums::HKEY_CURRENT_USER;

#[derive(Debug, Default, Clone)]
pub struct Settings {
    /// Percentage at or below which a device raises one low-battery alert.
    /// Zero disables battery alerts.
    pub alert_threshold: u8,
    /// Make the headset the default output while it is on, and return to the
    /// speaker when it turns off.
    pub auto_audio: bool,
    /// Endpoint id of the last non-headset output in use.
    pub speaker_endpoint: String,
    /// Endpoint ids never used as the speaker, such as a monitor without
    /// speakers. Set by hand in the registry.
    pub ignored_outputs: Vec<String>,
}

impl Settings {
    const KEY: &'static str = "Software\\GearTray";
    const RUN_KEY: &'static str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
    const RUN_VALUE: &'static str = "GearTray";
    /// Registry names used before the app was renamed from Battery Status.
    const OLD_KEY: &'static str = "Software\\BatteryStatus";
    const OLD_RUN_VALUE: &'static str = "BatteryStatus";

    pub fn startup_enabled() -> bool {
        winreg::RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(Self::RUN_KEY)
            .and_then(|key| key.get_value::<String, _>(Self::RUN_VALUE))
            .is_ok_and(|value| !value.is_empty())
    }

    /// Carry preferences and the startup entry over from Battery Status, once.
    fn migrate_from_old_name() -> Result<()> {
        let hkcu = winreg::RegKey::predef(HKEY_CURRENT_USER);
        if let Ok(old) = hkcu.open_subkey(Self::OLD_KEY) {
            let (key, _) = hkcu.create_subkey(Self::KEY).context("accessing registry key")?;
            for value in old.enum_values().flatten() {
                let (name, data) = value;
                if key.get_raw_value(&name).is_err() {
                    key.set_raw_value(&name, &data).context("copying old setting")?;
                }
            }
            hkcu.delete_subkey_all(Self::OLD_KEY).context("removing old settings")?;
            log::info!("Moved settings from Battery Status");
        }
        let run = hkcu.open_subkey(Self::RUN_KEY);
        if run.and_then(|key| key.get_value::<String, _>(Self::OLD_RUN_VALUE)).is_ok() {
            Self::set_startup(true)?;
            let (key, _) = hkcu.create_subkey(Self::RUN_KEY)?;
            key.delete_value(Self::OLD_RUN_VALUE).context("removing old startup entry")?;
        }
        Ok(())
    }

    pub fn set_startup(enabled: bool) -> Result<()> {
        let (key, _) = winreg::RegKey::predef(HKEY_CURRENT_USER)
            .create_subkey(Self::RUN_KEY)
            .context("opening Windows startup settings")?;
        if enabled {
            let executable = std::env::current_exe().context("locating executable")?;
            key.set_value(Self::RUN_VALUE, &format!("\"{}\"", executable.display()))
                .context("enabling startup")?;
        } else {
            match key.delete_value(Self::RUN_VALUE) {
                Ok(()) => {},
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
                Err(error) => return Err(error).context("disabling startup"),
            }
        }
        Ok(())
    }

    pub fn load() -> Result<Self> {
        if let Err(err) = Self::migrate_from_old_name() {
            log::warn!("Could not move settings from Battery Status: {err:?}");
        }
        let hkcu = winreg::RegKey::predef(HKEY_CURRENT_USER);
        let (key, _) = hkcu
            .create_subkey(Self::KEY)
            .context("accessing registry key")?;

        let alert_threshold = match key.get_value::<u32, _>("AlertThreshold") {
            Ok(value) => value.min(100) as u8,
            // Preserve the pre-threshold preference for upgrades.
            Err(_) => match key.get_value::<u32, _>("NotificationsEnabled") {
                Ok(value) if value != 0 => 10,
                _ => 0,
            },
        };

        let auto_audio = key
            .get_value::<u32, _>("AutoAudioOutput")
            .map_or(true, |value| value != 0);
        let speaker_endpoint = key.get_value::<String, _>("SpeakerEndpoint").unwrap_or_default();
        let ignored_outputs = key.get_value::<Vec<String>, _>("IgnoredOutputs").unwrap_or_default();

        let settings = Settings {
            alert_threshold,
            auto_audio,
            speaker_endpoint,
            ignored_outputs,
        };

        log::debug!("Loaded settings: {:?}", settings);
        Ok(settings)
    }

    pub fn save(&self) -> Result<()> {
        let hkcu = winreg::RegKey::predef(HKEY_CURRENT_USER);
        let (key, _) = hkcu
            .create_subkey(Self::KEY)
            .context("accessing registry key")?;

        key.set_value("AlertThreshold", &u32::from(self.alert_threshold.min(100)))
            .context("saving alert threshold to registry")?;
        key.set_value("AutoAudioOutput", &u32::from(self.auto_audio))
            .context("saving audio switching preference")?;
        key.set_value("SpeakerEndpoint", &self.speaker_endpoint)
            .context("saving speaker output")?;
        Ok(())
    }
}
