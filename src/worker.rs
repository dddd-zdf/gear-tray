//! Owns all device reads. Drivers that push are read only at startup, on
//! Refresh, and when they report a change; drivers that cannot push are
//! polled on a timer.

use std::{
    sync::{mpsc, Arc},
    thread,
    time::{Duration, Instant},
};

use crate::devices::{self, DeviceReading, Driver, PowerState, Push};

/// Poll interval for drivers whose devices never push changes.
const POLL_WITHOUT_PUSH: Duration = Duration::from_secs(10 * 60);

pub(crate) enum Wake {
    /// Read every device: startup or the panel's refresh button.
    Refresh,
    /// A driver (by index) reported something at this time.
    Push(usize, Push, Instant),
}

/// Start the worker and the drivers' push listeners. Readings go to
/// `readings_tx`, combined across drivers, whenever anything changes.
pub(crate) fn start(
    readings_tx: mpsc::Sender<Vec<DeviceReading>>,
) -> mpsc::Sender<Wake> {
    let drivers = devices::drivers();
    let (wake_tx, wake_rx) = mpsc::channel();
    for (index, driver) in drivers.iter().enumerate().filter(|(_, d)| d.pushes()) {
        let driver = Arc::clone(driver);
        let tx = wake_tx.clone();
        thread::spawn(move || {
            driver.watch(Arc::new(move |push| {
                let _ = tx.send(Wake::Push(index, push, Instant::now()));
            }))
        });
    }
    let _ = wake_tx.send(Wake::Refresh);
    thread::spawn(move || run(drivers, wake_rx, readings_tx));
    wake_tx
}

fn run(
    drivers: Vec<Arc<dyn Driver>>,
    wake_rx: mpsc::Receiver<Wake>,
    readings_tx: mpsc::Sender<Vec<DeviceReading>>,
) {
    let mut readings: Vec<Vec<DeviceReading>> = vec![Vec::new(); drivers.len()];
    // When each driver was last read; pushes older than that are already reflected.
    let mut read_at = vec![Instant::now(); drivers.len()];
    let timed = drivers.iter().any(|driver| !driver.pushes());
    let mut next_timed_poll = Instant::now() + POLL_WITHOUT_PUSH;
    loop {
        let wake = if timed {
            match wake_rx.recv_timeout(next_timed_poll.saturating_duration_since(Instant::now())) {
                Ok(wake) => Some(wake),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        } else {
            match wake_rx.recv() {
                Ok(wake) => Some(wake),
                Err(_) => return,
            }
        };
        match wake {
            Some(Wake::Refresh) => {
                for (index, driver) in drivers.iter().enumerate() {
                    read_at[index] = Instant::now();
                    readings[index] = read_confirmed(driver.as_ref(), &readings[index]);
                }
            }
            None => {
                for (index, driver) in drivers.iter().enumerate().filter(|(_, d)| !d.pushes()) {
                    read_at[index] = Instant::now();
                    readings[index] = read_confirmed(driver.as_ref(), &readings[index]);
                }
                next_timed_poll = Instant::now() + POLL_WITHOUT_PUSH;
            }
            Some(Wake::Push(index, push, at)) => {
                let driver = drivers[index].as_ref();
                match push {
                    // Off needs no query, which keeps sound switching instant.
                    Push::Link(false) if readings[index].len() <= 1 => {
                        for reading in &mut readings[index] {
                            reading.battery_percent = None;
                            reading.power = PowerState::Offline;
                        }
                    }
                    Push::Link(true) => {
                        read_at[index] = Instant::now();
                        readings[index] = read_after_connect(driver);
                    }
                    // A read that started after the push already reflects it;
                    // this also collapses the burst a mouse sends on connect.
                    _ if at < read_at[index] => continue,
                    Push::Link(false) | Push::Changed | Push::Reconnected => {
                        read_at[index] = Instant::now();
                        readings[index] = driver.poll();
                    }
                }
            }
        }
        if readings_tx.send(readings.concat()).is_err() {
            return;
        }
    }
}

fn connected(readings: &[DeviceReading]) -> bool {
    readings.iter().any(DeviceReading::is_connected)
}

/// Confirm a dropped wireless reading before clearing a previously visible
/// percentage; brief receiver wake-ups can reject a query.
fn read_confirmed(driver: &dyn Driver, previous: &[DeviceReading]) -> Vec<DeviceReading> {
    let readings = driver.poll();
    if !connected(previous) || connected(&readings) {
        return readings;
    }
    thread::sleep(Duration::from_millis(200));
    driver.poll()
}

/// A device that just turned on can take a moment to answer battery queries,
/// so retry for up to five seconds.
fn read_after_connect(driver: &dyn Driver) -> Vec<DeviceReading> {
    let mut readings = driver.poll();
    for _ in 0..10 {
        if connected(&readings) {
            break;
        }
        thread::sleep(Duration::from_millis(500));
        readings = driver.poll();
    }
    readings
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicU8, AtomicUsize, Ordering},
        Arc,
    };

    use super::*;
    use crate::devices::{DeviceKind, HidInterface};

    /// One headset whose battery level the test controls; counts reads.
    #[derive(Default)]
    struct FakeHeadset {
        level: AtomicU8,
        reads: AtomicUsize,
    }

    impl Driver for FakeHeadset {
        fn name(&self) -> &'static str {
            "fake"
        }
        fn poll(&self) -> Vec<DeviceReading> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            vec![DeviceReading {
                device_kind: DeviceKind::Headset,
                name: "Fake".into(),
                stable_id: "fake".into(),
                battery_percent: Some(self.level.load(Ordering::SeqCst)),
                power: PowerState::Unknown,
                sound_device: Some("Fake".into()),
            }]
        }
        fn pushes(&self) -> bool {
            true
        }
        fn interfaces(&self) -> Vec<HidInterface> {
            Vec::new()
        }
    }

    #[test]
    fn handles_pushes_without_needless_reads() {
        let fake = Arc::new(FakeHeadset::default());
        fake.level.store(80, Ordering::SeqCst);
        let (wake_tx, wake_rx) = mpsc::channel();
        let (readings_tx, readings_rx) = mpsc::channel();
        let before_start = Instant::now();
        wake_tx.send(Wake::Refresh).unwrap();
        // Older than the startup read, so already reflected: skipped.
        wake_tx.send(Wake::Push(0, Push::Changed, before_start)).unwrap();
        wake_tx.send(Wake::Push(0, Push::Link(false), Instant::now())).unwrap();
        drop(wake_tx);
        let driver: Arc<dyn Driver> = fake.clone();
        run(vec![driver], wake_rx, readings_tx);

        let sent: Vec<_> = readings_rx.iter().collect();
        assert_eq!(sent.len(), 2, "startup read, then off; the stale push sends nothing");
        assert_eq!(sent[0][0].battery_percent, Some(80));
        assert!(!sent[1][0].is_connected(), "off is applied without a read");
        assert_eq!(fake.reads.load(Ordering::SeqCst), 1);
    }
}
