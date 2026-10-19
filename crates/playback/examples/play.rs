//! Manual smoke test: `cargo run -p antistreaming-playback --example play -- <file> [seconds] [volume]`
//!
//! Plays through the default output device and prints every event as the UI would receive it,
//! prefixed with the time since launch. Pass volume 0 to run silently.

use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use antistreaming_playback::{PlaybackState, Player, PlayerEvent};

const POLL: Duration = Duration::from_millis(50);

/// Opening the device can take a while the first time.
const START_TIMEOUT: Duration = Duration::from_secs(10);

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let path = PathBuf::from(args.next().ok_or("usage: play <file> [seconds] [volume]")?);
    let seconds: u64 = args.next().map_or(Ok(5), |seconds| seconds.parse())?;
    let volume: f32 = args.next().map_or(Ok(0.5), |volume| volume.parse())?;

    let launched = Instant::now();
    let failed = Arc::new(AtomicBool::new(false));
    let player = {
        let failed = Arc::clone(&failed);
        Player::new(move |event| {
            if matches!(event, PlayerEvent::Error { .. }) {
                failed.store(true, Ordering::Relaxed);
            }
            let json = serde_json::to_string(&event).unwrap_or_default();
            println!("[{:>5} ms] {json}", launched.elapsed().as_millis());
        })?
    };
    player.set_volume(volume);
    player.play(1, path);

    let length = Duration::from_secs(seconds);
    let mut playing_since: Option<Instant> = None;
    loop {
        thread::sleep(POLL);
        if failed.load(Ordering::Relaxed) {
            break;
        }
        match (player.status().state, playing_since) {
            (PlaybackState::Playing, None) => playing_since = Some(Instant::now()),
            // The file ended before the requested time.
            (PlaybackState::Stopped, Some(_)) => break,
            (_, Some(since)) if since.elapsed() >= length => break,
            (_, None) if launched.elapsed() >= START_TIMEOUT => break,
            _ => {}
        }
    }

    println!("final status: {}", serde_json::to_string(&player.status())?);
    player.stop();
    Ok(())
}
