use battery_status::{audio_output, native_devices};

fn main() {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() == Some("--listen") {
        let seconds = args.next().and_then(|s| s.parse().ok()).unwrap_or(60);
        println!("Listening to headset reports for {seconds}s; nothing is sent.");
        native_devices::watch_headset_reports(std::time::Duration::from_secs(seconds));
        return;
    }
    println!("Battery Status — native HID diagnostics");
    println!("Battery queries only; no device settings are changed.\n");
    for device in native_devices::diagnostic_report() {
        println!("{device:#?}");
    }
    println!("\nReadings:");
    for reading in native_devices::poll_devices() {
        println!("{reading:#?}");
    }
    println!("\nSound outputs (* = default):");
    match audio_output::outputs() {
        Ok((outputs, current)) => {
            for output in outputs {
                let mark = if Some(&output.id) == current.as_ref() { "*" } else { " " };
                println!("{mark} {}  {}", output.name, output.id);
            }
        }
        Err(err) => println!("unavailable: {err:?}"),
    }
}
