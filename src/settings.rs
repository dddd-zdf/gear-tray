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
}

impl Settings {
    const KEY: &'static str = "Software\\BatteryStatus";
    const RUN_KEY: &'static str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";

    pub fn startup_enabled() -> bool {
        winreg::RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(Self::RUN_KEY)
            .and_then(|key| key.get_value::<String, _>("BatteryStatus"))
            .is_ok_and(|value| !value.is_empty())
    }

    pub fn set_startup(enabled: bool) -> Result<()> {
        let (key, _) = winreg::RegKey::predef(HKEY_CURRENT_USER)
            .create_subkey(Self::RUN_KEY)
            .context("opening Windows startup settings")?;
        if enabled {
            let executable = std::env::current_exe().context("locating executable")?;
            key.set_value("BatteryStatus", &format!("\"{}\"", executable.display()))
                .context("enabling startup")?;
        } else {
            match key.delete_value("BatteryStatus") {
                Ok(()) => {},
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
                Err(error) => return Err(error).context("disabling startup"),
            }
        }
        Ok(())
    }

    pub fn load() -> Result<Self> {
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

        let settings = Settings {
            alert_threshold,
            auto_audio,
            speaker_endpoint,
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
