# Gear Tray

A small Windows tray app for HyperX Cloud III S Wireless and Logitech PRO X SUPERLIGHT 2 DEX: battery levels for both, and sound that follows the headset. Formerly Battery Status. Adapted from [aarol/headset-battery-indicator](https://github.com/aarol/headset-battery-indicator), with device-specific tray icons and a compact battery panel.

## Run

Open `dist/GearTray.exe`. It runs in the notification area without a main window. Windows may initially put the icons in the tray overflow menu.

There is one icon for the headset and one for the mouse. Left-click either icon to open the compact battery panel. Set the alert threshold with the minus/plus buttons, which adjust it by 10 percentage points per click, or click the value and type an exact percentage from 0 to 100. 0% turns alerts off; a device alerts once when its battery is at or below the threshold. Preferences are saved automatically. Refresh is in the bottom-right corner. Right-click either tray icon to toggle **Start with Windows**, toggle **Switch sound to headset**, or select **Quit** to exit. Logs are stored in gear-tray.log beside the executable. Disconnected or unknown readings are not shown as a measured zero.

There is no polling timer. Both devices push changes on their own, and the tray updates the moment they arrive:

| Device | Push | Meaning |
|---|---|---|
| Headset dongle | `0d 02 03 00 0c 01` / `… 0c 00` | headset on / off |
| Headset dongle | `0d 02 03 00 01 <percent>` | new battery level |
| Logitech receiver | `10 <slot> 41 …` (byte 4 bit 6 set = link lost) | mouse on / off |
| Mouse | HID++ 2.0 event, software id 0 (battery feature `0x1004`) | battery or charging change; the app re-reads the mouse |

Devices are read in full only at startup, on Refresh, and when the dongle or receiver is plugged back in. These reports were observed on this PC with `GearDiagnostics.exe --listen`; they are not from published documentation.

**Switch sound to headset** (on by default) makes the headset the default Windows output the moment it turns on, and returns to the speaker when it turns off. The speaker is whichever non-headset output was in use before; without one, the first ordinary output (Bluetooth hands-free outputs are skipped). The app acts only when the headset turns on or off, so an output you pick by hand stays until the next change. The headset's audio device stays "active" in Windows whenever the dongle is plugged in, so on/off comes from the dongle's own push report rather than from Windows audio events.

`dist/GearDiagnostics.exe` prints native HID detection and readings in a console. It uses the same device module as the UI. Run it from PowerShell to keep the output visible:

```powershell
./dist/GearDiagnostics.exe
```

It also lists sound outputs (`*` marks the default). `./dist/GearDiagnostics.exe --listen 60` prints every report the headset dongle and the Logitech receiver send on their own for 60 seconds, without sending anything.

No vendor application or administrator account is intended to be required for normal HID access. The supported path is the USB wireless receiver (and supported Logitech wired HID connections); Bluetooth is not implemented.

The headset reports a battery percentage but its referenced protocol does not document a charging flag. Its percentage is displayed normally without a charging claim. Alerts are disabled at a threshold of 0%. The app registers a per-user Start menu shortcut for Windows toast delivery. Windows notification settings still control whether banners appear.

## Modules

- `src/native_devices.rs`: device discovery, HID transport, battery protocol handling and plain reading types. No tray or window dependencies in this module.
- `src/lib.rs`: starts the device worker and the push listeners for both devices, and passes readings to the UI.
- `src/audio_output.rs`: lists Windows sound outputs, changes the default, and decides which output fits the headset state.
- `src/app.rs`: application controller, tray lifecycle, readings and panel actions.
- `src/panel.rs`: popup layout, drawing and input handling.
- `src/tray_icons.rs`: device glyph and battery percentage images for the tray.
- `src/notify.rs`, `src/settings.rs`: battery alerts and saved preferences.
- `src/bin/diagnostics.rs`: console consumer of the device module.

To add another device, implement its battery query behind the device module and return a reading. Protocol code should never manipulate tray icons; UI code should never open HID handles.

## Build and test

Requires Windows, stable Rust with the MSVC target, and Visual Studio C++ Build Tools plus the Windows SDK. Fetch submodules after cloning:

```powershell
git submodule update --init --recursive
./build.ps1 -Test
```

The script uses the workspace-local Rust installation in `.tools` when present, otherwise the system Cargo. It generates a release app and diagnostics executable in `dist`. Use Start with Windows in the right-click menu to enable or disable startup at sign-in. It is off by default.

## Credits

This project builds on the work of the following authors and communities:

- **[aarol/headset-battery-indicator](https://github.com/aarol/headset-battery-indicator)** by Aaro Luomanen — the original application foundation and UI inspiration. Gear Tray retains the original Git history and adapts its Windows tray infrastructure.
- **[auto94/HyperX-Cloud-2-Battery-Monitor](https://github.com/auto94/HyperX-Cloud-2-Battery-Monitor)** — the reference for HyperX headset battery communication, including the Cloud III S Wireless protocol used here.
- **[andyvorld/LGSTrayBattery](https://github.com/andyvorld/LGSTrayBattery)** — the reference for Logitech native HID/HID++ device discovery and battery queries, used to develop support for the PRO X 2 DEX.

Thank you to these projects for making their source code and protocol implementations available. This is an independent adaptation, not an official HyperX or Logitech application.

See [Third-party notices](THIRD-PARTY-NOTICES.md) for the reference revisions and license notices. The original project's documentation is preserved in [Upstream README](docs/UPSTREAM-README.md).

## Sources and licensing

Live verification on this PC confirmed that HyperX `03F0:06BE` and Logitech LIGHTSPEED `046D:C54D` are discovered through their native HID protocols. The mouse sends 20-byte HID++ replies on the long HID collection even for 7-byte requests on the short collection; both channels are handled. Battery values are point-in-time readings and can change as the devices are used.

This modified application is licensed under [GPL-3.0-only](LICENSE). Preserve the license and provide corresponding source when redistributing it.
