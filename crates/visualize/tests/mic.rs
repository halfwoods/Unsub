use std::sync::{Arc, Mutex};
use std::time::Duration;

use antistreaming_visualize::{Analyzer, Error, MicDevice, MicListener};

#[test]
fn the_listener_can_be_kept_in_shared_application_state() {
    fn shared<T: Send + Sync>() {}
    shared::<MicListener>();
    shared::<MicDevice>();
}

#[test]
fn a_microphone_that_does_not_exist_is_an_error_not_a_panic() {
    let analyzer = Arc::new(Mutex::new(Analyzer::new(44_100, 1)));

    let result = MicListener::start(Some("no such microphone, ever".to_owned()), analyzer);

    // Where there is no audio system at all this fails in another way, which is as good.
    assert!(
        matches!(
            result,
            Err(Error::DeviceNotFound(_) | Error::Audio(_) | Error::NoInputDevice)
        ),
        "{:?}",
        result.err()
    );
}

/// Opens the system default microphone for real. On macOS this brings up the permission prompt
/// the first time, and the run must be from an app (or terminal) that may ask for it.
#[test]
#[ignore = "needs a microphone and, on macOS, microphone permission"]
fn a_real_microphone_feeds_the_analyzer() {
    let analyzer = Arc::new(Mutex::new(Analyzer::new(44_100, 1)));

    let listener =
        MicListener::start(None, Arc::clone(&analyzer)).expect("open the default microphone");
    std::thread::sleep(Duration::from_millis(1500));

    assert!(listener.is_running());
    let frame = analyzer.lock().unwrap().frame_at(listener.now());
    assert!(frame.is_some(), "no frame from {}", listener.device_name());
    drop(listener);
    println!("devices: {:?}", MicListener::devices());
}
