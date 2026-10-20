mod support;

use support::*;
use unsub_visualize::Analyzer;

/// Alone in its own test binary, so no other test competes for the processor. Relies on the
/// workspace's `[profile.dev.package.unsub-visualize]`; unoptimized it manages about 30.
#[test]
fn analysis_runs_at_least_fifty_times_faster_than_real_time() {
    let seconds = 20.0;
    let signal = stereo(&noise(seconds, 48_000, 13));
    let mut analyzer = Analyzer::new(48_000, 2);

    let started = std::time::Instant::now();
    for (n, block) in signal.chunks(2 * 480).enumerate() {
        analyzer.push(block, secs(n as f64 * 0.01));
        std::hint::black_box(analyzer.frame_at(secs(n as f64 * 0.01)));
    }
    let took = started.elapsed().as_secs_f32();

    println!(
        "{seconds} s of audio took {took} s: {}x real time",
        seconds / took
    );
    assert!(took < seconds / 50.0, "{seconds} s of audio took {took} s");
}
