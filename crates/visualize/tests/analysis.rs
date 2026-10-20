mod support;

use antistreaming_visualize::{Analyzer, BANDS};
use support::*;

#[test]
fn a_sine_lights_the_band_at_its_frequency_at_every_common_rate() {
    for rate in [22_050, 32_000, 44_100, 48_000, 96_000, 192_000] {
        for hz in [50.0, 110.0, 440.0, 1000.0, 3000.0, 8000.0, 14_000.0] {
            if hz > rate as f32 * 0.45 {
                continue;
            }
            let mut analyzer = Analyzer::new(rate, 1);
            analyzer.push(&sine(hz, 0.5, 1.0, rate), secs(0.0));

            let frame = at(&analyzer, 0.5);

            let band = loudest_band(&frame);
            // The FFT cannot place a tone closer than half a bin.
            let tolerance = 0.12_f32.max(0.1 + 0.5 * (rate as f32 / 2048.0) / hz);
            assert!(
                (band_hz(band, rate) / hz).ln().abs() < tolerance,
                "{hz} Hz at {rate}: band {band} is {} Hz",
                band_hz(band, rate)
            );
            assert!(
                frame.bands[band] > 0.5,
                "{hz} Hz at {rate}: {}",
                frame.bands[band]
            );
            let bin = rate as f32 / 2048.0;
            let far_bands = (0..BANDS).filter(|&other| {
                other.abs_diff(band) >= 6 && (band_hz(other, rate) - hz).abs() > 6.0 * bin
            });
            for far in far_bands {
                assert!(
                    frame.bands[far] < 0.2,
                    "{hz} Hz at {rate}: band {far} {}",
                    frame.bands[far]
                );
            }
        }
    }
}

#[test]
fn a_sweep_is_followed_up_the_bands() {
    let rate = RATE;
    let seconds = 2.0_f32;
    let (from, to) = (100.0_f32, 10_000.0_f32);
    // Exponential sweep: the frequency at time t is from * (to / from)^(t / seconds).
    let k = (to / from).ln() / seconds;
    let mut phase = 0.0_f32;
    let sweep: Vec<f32> = (0..(seconds * rate as f32) as usize)
        .map(|n| {
            let hz = from * (k * n as f32 / rate as f32).exp();
            phase += TAU_F * hz / rate as f32;
            0.5 * phase.sin()
        })
        .collect();
    let run = play(&sweep, 1, rate, 512, 0.0, 0.1);

    for step in 3..17 {
        let t = step as f32 * 0.1;
        let expected = from * (k * t).exp();
        let band = loudest_band(run.near(f64::from(t)));
        assert!(
            (band_hz(band, rate) / expected).ln().abs() < 0.2,
            "at {t} s expected {expected} Hz, band is {} Hz",
            band_hz(band, rate)
        );
    }
}

const TAU_F: f32 = std::f32::consts::TAU;

#[test]
fn silence_gives_zeros_and_no_beats() {
    let run = play(&stereo(&silence(4.0, RATE)), 2, RATE, 512, 0.0, 0.1);

    assert!(run.frames.len() > 150);
    for frame in run.frames.values() {
        assert!(frame.bands.iter().all(|&band| band == 0.0));
        assert!(frame.peaks.iter().all(|&peak| peak == 0.0));
        assert!(
            frame
                .waveform_l
                .iter()
                .chain(&frame.waveform_r)
                .all(|&v| v == 0.0)
        );
        assert_eq!(
            (frame.rms, frame.onset, frame.beat_strength),
            (0.0, 0.0, 0.0)
        );
        assert!(!frame.beat);
        assert_eq!(frame.tempo_bpm, None);
    }
}

#[test]
fn bands_rise_quickly_and_fall_back_over_a_few_hundred_milliseconds() {
    let mut signal = silence(0.3, RATE);
    signal.extend(sine(1000.0, 0.9, 0.5, RATE));
    signal.extend(silence(1.2, RATE));
    let run = play(&signal, 1, RATE, 512, 0.0, 0.1);
    let band = loudest_band(run.near(0.6));
    let level = |t: f64| run.near(t).bands[band];

    let steady = level(0.7);
    assert!(steady > 0.7, "{steady}");
    assert!(
        level(0.3 + 0.07) > 0.85 * steady,
        "rises within about 50 ms of the tone"
    );
    assert!(
        level(0.8 + 0.1) > 0.3 * steady,
        "has not vanished 100 ms after"
    );
    assert!(
        level(0.8 + 0.6) < 0.1,
        "has gone 600 ms after: {}",
        level(1.4)
    );

    let falling = run.near(0.8 + 0.12);
    assert!(
        falling.peaks[band] > falling.bands[band] + 0.05,
        "the cap stays up while the bar falls"
    );
    assert!(
        run.near(0.83).peaks[band] > steady - 0.01,
        "and holds after the bar has started down"
    );
    assert_eq!(run.near(1.9).peaks[band], 0.0, "and falls back in the end");
}

#[test]
fn every_value_stays_in_its_range_and_peaks_cap_the_bands() {
    let mut loud = noise(3.0, RATE, 3);
    for (n, sample) in loud.iter_mut().enumerate() {
        *sample *= if (n / 11_000) % 2 == 0 { 3.0 } else { 0.01 };
    }
    let run = play(&stereo(&loud), 2, RATE, 700, 5.0, 0.1);

    assert!(run.frames.len() > 100);
    for frame in run.frames.values() {
        assert!(
            frame
                .bands
                .iter()
                .chain(&frame.peaks)
                .all(|v| (0.0..=1.0).contains(v))
        );
        assert!(
            frame
                .bands
                .iter()
                .zip(&frame.peaks)
                .all(|(band, peak)| peak >= band)
        );
        assert!(
            frame
                .waveform_l
                .iter()
                .chain(&frame.waveform_r)
                .all(|v| (-1.0..=1.0).contains(v))
        );
        assert!((0.0..=1.0).contains(&frame.rms) && (0.0..=1.0).contains(&frame.onset));
        assert!((0.0..=1.0).contains(&frame.beat_strength));
    }
}

#[test]
fn the_waveforms_keep_the_channels_apart() {
    let tone = sine(440.0, 0.8, 1.0, RATE);
    let none = silence(1.0, RATE);
    for (left, right) in [(&tone, &none), (&none, &tone)] {
        let mut analyzer = Analyzer::new(RATE, 2);
        analyzer.push(&interleave(left, right), secs(0.0));

        let frame = at(&analyzer, 0.5);

        let (loud, quiet) = if left == &tone {
            (&frame.waveform_l, &frame.waveform_r)
        } else {
            (&frame.waveform_r, &frame.waveform_l)
        };
        assert!(loud.iter().fold(0.0_f32, |m, v| m.max(v.abs())) > 0.7);
        assert!(quiet.iter().all(|&v| v == 0.0));
    }
}

#[test]
fn opposite_channels_stay_opposite_on_one_time_axis_and_mono_is_both() {
    let tone = sine(300.0, 0.6, 1.0, RATE);
    let inverted: Vec<f32> = tone.iter().map(|v| -v).collect();
    let mut analyzer = Analyzer::new(RATE, 2);
    analyzer.push(&interleave(&tone, &inverted), secs(0.0));
    let frame = at(&analyzer, 0.5);
    for (l, r) in frame.waveform_l.iter().zip(&frame.waveform_r) {
        assert!((l + r).abs() < 1e-4);
    }

    let mut mono = Analyzer::new(RATE, 1);
    mono.push(&tone, secs(0.0));
    let frame = at(&mono, 0.5);
    assert_eq!(frame.waveform_l, frame.waveform_r);
    assert!(frame.waveform_l.iter().any(|&v| v.abs() > 0.5));
}

#[test]
fn a_steady_tone_is_drawn_in_the_same_place_every_frame() {
    let mut analyzer = Analyzer::new(RATE, 1);
    analyzer.push(&sine(441.0, 0.7, 1.5, RATE), secs(0.0));

    let first = at(&analyzer, 0.6);
    for step in 0..40 {
        let frame = at(&analyzer, 0.6 + step as f64 * 0.0167);
        // It begins on a rising zero crossing...
        assert!(frame.waveform_l[0].abs() < 0.15, "{}", frame.waveform_l[0]);
        assert!(frame.waveform_l[1] >= frame.waveform_l[0]);
        // ...so it overlays the first frame.
        for (a, b) in frame.waveform_l.iter().zip(&first.waveform_l) {
            assert!((a - b).abs() < 0.1, "step {step}");
        }
    }
}

#[test]
fn a_frame_shows_the_audio_of_its_own_moment() {
    let burst = sine(1000.0, 0.8, 0.1, RATE);
    let signal = with_bursts(1.3, RATE, &[0.7], &burst);
    let mut analyzer = Analyzer::new(RATE, 1);
    analyzer.push(&signal, secs(0.0));
    let peak = |t: f64| {
        at(&analyzer, t)
            .waveform_l
            .iter()
            .fold(0.0_f32, |m, v| m.max(v.abs()))
    };

    assert!(peak(0.6) < 0.01);
    assert!(peak(0.75) > 0.5);
    assert!(peak(0.9) < 0.01);
}

#[test]
fn frame_at_returns_the_frame_nearest_the_time_asked() {
    let mut analyzer = Analyzer::new(RATE, 2);
    assert!(
        analyzer.frame_at(secs(0.0)).is_none(),
        "nothing before any audio"
    );
    analyzer.push(&stereo(&noise(1.0, RATE, 5)), secs(10.0));

    for step in 0..30 {
        let t = 10.1 + step as f64 * 0.0213;
        let frame = at(&analyzer, t);
        assert!(
            (frame.t_ms as f64 - t * 1000.0).abs() <= 1000.0 * 735.0 / 44_100.0 / 2.0 + 1.0,
            "asked {t}, got {} ms",
            frame.t_ms
        );
    }
    assert!(analyzer.frame_at(secs(9.0)).is_none(), "before the audio");
    assert!(analyzer.frame_at(secs(12.0)).is_none(), "after it");
    // The newest frame is a little behind the newest audio, and is still found.
    assert!(analyzer.frame_at(secs(10.95)).is_some());
}

#[test]
fn between_two_frames_the_continuous_values_are_blended() {
    let mut modulated = noise(1.0, RATE, 9);
    for (n, sample) in modulated.iter_mut().enumerate() {
        *sample *= if (n / 3000) % 2 == 0 { 0.8 } else { 0.05 };
    }
    let mut analyzer = Analyzer::new(RATE, 1);
    analyzer.push(&modulated, secs(0.0));
    let period = 735.0 / f64::from(RATE);

    let mut blended = 0;
    for step in 0..40 {
        let base = at(&analyzer, 0.3 + step as f64 * 0.01).t_ms as f64 / 1000.0;
        let (a, b) = (at(&analyzer, base), at(&analyzer, base + period));
        let middle = at(&analyzer, base + period / 2.0);
        let (low, high) = (a.rms.min(b.rms), a.rms.max(b.rms));
        assert!(middle.rms >= low - 0.002 && middle.rms <= high + 0.002);
        assert!(
            [a.t_ms, b.t_ms].contains(&middle.t_ms),
            "named by a real frame"
        );
        if high - low > 0.01
            && (middle.rms - low).abs() > 0.002
            && (high - middle.rms).abs() > 0.002
        {
            blended += 1;
        }
    }
    assert!(
        blended > 0,
        "some midpoints must lie strictly between their neighbours"
    );
}

#[test]
fn a_jump_in_time_forgets_what_came_before_but_jitter_does_not() {
    let tone = stereo(&sine(500.0, 0.5, 1.0, RATE));
    let mut analyzer = Analyzer::new(RATE, 2);
    analyzer.push(&tone, secs(0.0));
    assert!(analyzer.frame_at(secs(0.5)).is_some());
    // A short block after a gap: if the old frames were kept they would still answer for 0.5 s.
    analyzer.push(&tone[..2 * 13_230], secs(5.0));
    assert!(
        analyzer.frame_at(secs(0.5)).is_none(),
        "a 4 s gap starts afresh"
    );
    assert!(analyzer.frame_at(secs(5.15)).is_some());
    assert!(analyzer.frame_at(secs(1.0)).is_none());

    // Blocks stamped a few milliseconds off, as a wall clock would, carry on without a break.
    let mut jittery = Analyzer::new(RATE, 2);
    for (n, block) in tone.chunks(2 * 441).enumerate() {
        let wobble = if n % 2 == 0 { 0.004 } else { -0.004 };
        jittery.push(block, secs((n as f64 * 0.01 + wobble).max(0.0)));
    }
    assert!(
        jittery.frame_at(secs(0.15)).is_some(),
        "the start is still there"
    );
    assert!(jittery.frame_at(secs(0.85)).is_some());
}

#[test]
fn how_the_audio_is_cut_into_blocks_makes_no_difference() {
    let mut mixed = noise(1.5, RATE, 21);
    for (n, sample) in mixed.iter_mut().enumerate() {
        *sample =
            *sample * 0.1 + 0.4 * (std::f32::consts::TAU * 200.0 * n as f32 / RATE as f32).sin();
    }
    let signal = stereo(&mixed);
    let whole = {
        let mut analyzer = Analyzer::new(RATE, 2);
        analyzer.push(&signal, secs(0.0));
        analyzer
    };
    for block in [1usize, 7, 333, 4096] {
        let mut analyzer = Analyzer::new(RATE, 2);
        for (n, chunk) in signal.chunks(block * 2).enumerate() {
            analyzer.push(chunk, secs((n * block) as f64 / f64::from(RATE)));
        }
        for t in [0.8, 1.0, 1.3] {
            let (a, b) = (at(&whole, t), at(&analyzer, t));
            assert_eq!(
                (&a.bands, &a.peaks, &a.waveform_l),
                (&b.bands, &b.peaks, &b.waveform_l),
                "block {block}"
            );
            assert_eq!(
                (a.rms, a.onset, a.beat),
                (b.rms, b.onset, b.beat),
                "block {block}"
            );
            assert!(a.t_ms.abs_diff(b.t_ms) <= 1);
        }
    }
}

#[test]
fn a_frame_serializes_with_the_names_the_renderer_reads() {
    let mut analyzer = Analyzer::new(RATE, 1);
    analyzer.push(&sine(440.0, 0.5, 0.5, RATE), secs(0.0));

    let json = serde_json::to_value(at(&analyzer, 0.25)).unwrap();

    let object = json.as_object().unwrap();
    let mut names: Vec<_> = object.keys().map(String::as_str).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "bands",
            "beat",
            "beatStrength",
            "onset",
            "peaks",
            "rms",
            "tMs",
            "tempoBpm",
            "waveformL",
            "waveformR"
        ]
    );
    assert_eq!(object["bands"].as_array().unwrap().len(), 64);
    assert_eq!(object["peaks"].as_array().unwrap().len(), 64);
    assert_eq!(object["waveformL"].as_array().unwrap().len(), 256);
    assert_eq!(object["waveformR"].as_array().unwrap().len(), 256);
    assert!(object["tempoBpm"].is_null());
    assert!(object["tMs"].is_u64());
}

#[test]
fn a_frame_is_small_and_short_enough_for_the_webview_to_take_directly() {
    // The worst case is a loud, busy signal: every waveform point and band is a long fraction.
    let mut analyzer = Analyzer::new(RATE, 2);
    analyzer.push(&stereo(&noise(2.0, RATE, 5)), secs(0.0));

    let json = serde_json::to_string(&at(&analyzer, 1.0)).unwrap();

    assert!(json.len() < 8192, "{} bytes", json.len());
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    for name in ["bands", "peaks", "waveformL", "waveformR"] {
        for level in value[name].as_array().unwrap() {
            let level = level.as_f64().unwrap();
            assert!(
                ((level * 1e4).round() - level * 1e4).abs() < 1e-6,
                "{name}: {level}"
            );
        }
    }
}

#[test]
fn beat_between_finds_beats_the_frames_asked_for_did_not_have_yet() {
    let times = beat_times(120.0, 0.25, 12.0);
    let signal = with_bursts(12.0, RATE, &times, &kick(RATE));
    // Asking for the frame at the end of each block, as a mic does, always gets the newest frame,
    // and a frame only learns it is a beat from the one after it.
    let run = play(&stereo(&signal), 2, RATE, 512, 0.0, 0.0);
    let analyzer = &run.analyzer;
    let seen = run.frames.values().filter(|frame| frame.beat).count();
    assert!(
        seen <= 2,
        "{seen} beats were on frames as they were asked for"
    );

    // The analyzer keeps the last second of frames, so look at the clicks in it.
    let mut strongest = 0.0_f32;
    for &click in times.iter().filter(|&&t| t > 11.0) {
        let click = f64::from(click);
        let found = analyzer.beat_between(secs(click - 0.02), secs(click + 0.02));
        let strength = found.unwrap_or_else(|| panic!("no beat near {click} s"));
        assert!(strength > 0.0 && strength <= 1.0);
        assert_eq!(
            analyzer.beat_between(secs(click + 0.1), secs(click + 0.4)),
            None,
            "nothing between the clicks"
        );
        strongest = strongest.max(strength);
    }
    assert_eq!(
        analyzer.beat_between(secs(11.0), secs(12.0)),
        Some(strongest)
    );
    assert_eq!(analyzer.beat_between(secs(12.0), secs(11.0)), None);
}

#[test]
fn beats_are_known_up_to_the_frame_before_the_newest() {
    let mut analyzer = Analyzer::new(RATE, 1);
    assert_eq!(analyzer.beats_known_until(), None);
    analyzer.push(&sine(440.0, 0.5, 1.0, RATE), secs(0.0));

    let newest = at(&analyzer, 1.0);
    let known = analyzer.beats_known_until().expect("two frames by now");

    let known_ms = known.as_millis() as u64;
    assert!(known_ms < newest.t_ms, "{known_ms} against {}", newest.t_ms);
    assert!(known_ms + 40 > newest.t_ms, "one frame behind, not more");
}

#[test]
fn a_click_track_gives_beats_on_the_clicks_and_the_tempo() {
    for rate in [32_000, 44_100, 48_000, 96_000, 192_000] {
        for (name, burst) in [("kick", kick(rate)), ("click", click(rate))] {
            let times = beat_times(120.0, 0.25, 16.0);
            let signal = with_bursts(16.0, rate, &times, &burst);
            let run = play(&stereo(&signal), 2, rate, 512, 0.0, 0.15);

            let beats: Vec<f64> = run
                .frames
                .values()
                .filter(|frame| frame.beat)
                .map(|frame| frame.t_ms as f64 / 1000.0)
                .collect();
            // The adaptive threshold needs a moment; after the first two seconds every click counts.
            for &click in times.iter().filter(|&&t| t > 2.0) {
                let nearest = beats
                    .iter()
                    .map(|&beat| (beat - f64::from(click)).abs())
                    .fold(f64::MAX, f64::min);
                assert!(
                    nearest <= 0.020,
                    "{name} at {rate}: click at {click} s has no beat within 20 ms ({nearest})"
                );
            }
            let stray = beats
                .iter()
                .filter(|&&beat| {
                    times
                        .iter()
                        .all(|&click| (beat - f64::from(click)).abs() > 0.020)
                })
                .count();
            assert!(
                stray <= 1,
                "{name} at {rate}: {stray} of {} beats are off the clicks",
                beats.len()
            );
            let strengths: Vec<f32> = run
                .frames
                .values()
                .filter(|f| f.beat)
                .map(|f| f.beat_strength)
                .collect();
            assert!(
                strengths.iter().all(|s| (0.0..=1.0).contains(s))
                    && strengths.iter().any(|&s| s > 0.3)
            );
            assert!(
                run.frames
                    .values()
                    .filter(|f| !f.beat)
                    .all(|f| f.beat_strength == 0.0)
            );

            let tempo = run
                .frames
                .values()
                .last()
                .unwrap()
                .tempo_bpm
                .expect("tempo settles");
            assert!((tempo - 120.0).abs() < 2.0, "{name} at {rate}: {tempo} BPM");
            let first_tempo = run
                .frames
                .values()
                .find(|f| f.tempo_bpm.is_some())
                .unwrap()
                .t_ms;
            assert!(
                first_tempo < 9_000,
                "{name} at {rate}: tempo took until {first_tempo} ms"
            );
        }
    }
}

#[test]
fn beats_land_within_20_ms_whatever_their_phase_against_the_frames() {
    for bpm in [97.0_f32, 128.0, 133.0, 151.0] {
        for (name, burst) in [("kick", kick(RATE)), ("click", click(RATE))] {
            let times = beat_times(bpm, 0.2, 14.0);
            let run = play(
                &with_bursts(14.0, RATE, &times, &burst),
                1,
                RATE,
                512,
                0.0,
                0.15,
            );

            let beats: Vec<f64> = run
                .frames
                .values()
                .filter(|f| f.beat)
                .map(|f| f.t_ms as f64 / 1000.0)
                .collect();
            for &click in times.iter().filter(|&&t| t > 2.0) {
                let nearest = beats
                    .iter()
                    .map(|&b| (b - f64::from(click)).abs())
                    .fold(f64::MAX, f64::min);
                assert!(
                    nearest <= 0.020,
                    "{name} at {bpm} BPM: click at {click} s is {nearest} s from a beat"
                );
            }
        }
    }
}

#[test]
fn kicks_over_a_bass_line_and_hiss_are_still_beats() {
    let times = beat_times(128.0, 0.2, 14.0);
    let mut signal = with_bursts(
        14.0,
        RATE,
        &times,
        &kick(RATE).iter().map(|v| v * 0.7).collect::<Vec<_>>(),
    );
    let hiss = noise(14.0, RATE, 17);
    let bass = sine(98.0, 0.15, 14.0, RATE);
    for ((sample, noise), bass) in signal.iter_mut().zip(&hiss).zip(&bass) {
        *sample += noise * 0.05 + bass;
    }
    let run = play(&signal, 1, RATE, 512, 0.0, 0.15);

    let beats: Vec<f64> = run
        .frames
        .values()
        .filter(|f| f.beat)
        .map(|f| f.t_ms as f64 / 1000.0)
        .collect();
    let settled: Vec<f32> = times.iter().copied().filter(|&t| t > 2.0).collect();
    let caught = settled
        .iter()
        .filter(|&&click| {
            beats
                .iter()
                .any(|&beat| (beat - f64::from(click)).abs() <= 0.020)
        })
        .count();
    assert!(
        caught as f32 >= 0.9 * settled.len() as f32,
        "{caught} of {} kicks",
        settled.len()
    );
    assert!(
        beats.len() <= times.len() + 2,
        "{} beats for {} kicks",
        beats.len(),
        times.len()
    );
}

#[test]
fn other_tempi_are_found_too() {
    for bpm in [75.0_f32, 90.0, 140.0, 174.0] {
        let signal = with_bursts(18.0, RATE, &beat_times(bpm, 0.1, 18.0), &kick(RATE));
        let run = play(&signal, 1, RATE, 1024, 0.0, 0.15);

        let tempo = run.frames.values().last().unwrap().tempo_bpm;
        let tempo = tempo.unwrap_or_else(|| panic!("{bpm} BPM: no tempo"));
        assert!(
            (tempo / bpm - 1.0).abs() < 0.03,
            "{bpm} BPM heard as {tempo}"
        );
    }
}

#[test]
fn two_hits_within_a_moment_of_each_other_are_one_beat() {
    let hit = click(RATE);
    let times: Vec<f32> = beat_times(100.0, 0.3, 14.0)
        .into_iter()
        .flat_map(|t| [t, t + 0.1])
        .collect();
    let run = play(
        &with_bursts(14.0, RATE, &times, &hit),
        1,
        RATE,
        512,
        0.0,
        0.15,
    );

    let beats = run.frames.values().filter(|f| f.beat).count();
    let pairs = times.len() / 2;
    assert!(
        beats <= pairs + 2,
        "{beats} beats for {pairs} pairs of hits"
    );
    assert!(
        beats as f32 >= 0.8 * pairs as f32,
        "{beats} beats for {pairs} pairs of hits"
    );
}

#[test]
fn a_steady_tone_and_noise_have_no_beats_and_no_tempo() {
    for signal in [
        sine(80.0, 0.5, 12.0, RATE),
        noise(12.0, RATE, 11).iter().map(|v| v * 0.3).collect(),
    ] {
        let run = play(&signal, 1, RATE, 512, 0.0, 0.15);

        let beats = run.frames.values().filter(|f| f.beat).count();
        assert!(beats <= 2, "{beats} beats in 12 s of steady sound");
        assert_eq!(run.frames.values().last().unwrap().tempo_bpm, None);
    }
}

#[test]
fn odd_input_never_produces_odd_output() {
    let mut analyzer = Analyzer::new(RATE, 2);
    analyzer.push(&[], secs(0.0));
    analyzer.push(&[0.5], secs(0.0));
    assert!(analyzer.frame_at(secs(0.0)).is_none());

    let mut poisoned = stereo(&sine(300.0, 0.5, 1.0, RATE));
    for index in (0..poisoned.len()).step_by(997) {
        poisoned[index] = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1e30][index % 4];
    }
    // And a half sample-frame left over at the end.
    poisoned.push(0.25);
    analyzer.push(&poisoned, secs(0.0));

    let frame = at(&analyzer, 0.5);
    let finite = frame
        .bands
        .iter()
        .chain(&frame.peaks)
        .chain(&frame.waveform_l)
        .chain(&frame.waveform_r)
        .chain([&frame.rms, &frame.onset, &frame.beat_strength])
        .all(|value| value.is_finite());
    assert!(finite);
    assert!(frame.waveform_l.iter().all(|v| v.abs() <= 1.0) && frame.rms <= 1.0);
}

#[test]
fn any_channel_count_and_rate_works() {
    for (rate, channels) in [(8_000, 1u16), (16_000, 2), (44_100, 6), (192_000, 2)] {
        let tone = sine(1000.0, 0.5, 1.0, rate);
        let mut interleaved = Vec::new();
        for &sample in &tone {
            interleaved.extend(std::iter::repeat_n(sample, usize::from(channels)));
        }
        let mut analyzer = Analyzer::new(rate, channels);
        analyzer.push(&interleaved, secs(0.0));

        let frame = at(&analyzer, 0.5);

        assert_eq!(
            (analyzer.sample_rate(), analyzer.channels()),
            (rate, channels)
        );
        assert!(
            (band_hz(loudest_band(&frame), rate) / 1000.0).ln().abs() < 0.2,
            "{rate} Hz"
        );
        assert!(frame.waveform_l.iter().any(|v| v.abs() > 0.3));
    }
    // Nonsense is made sensible rather than rejected.
    let analyzer = Analyzer::new(0, 0);
    assert_eq!((analyzer.sample_rate(), analyzer.channels()), (8_000, 1));
}

#[test]
fn a_long_block_is_taken_in_one_go_and_only_the_last_second_is_kept() {
    let mut analyzer = Analyzer::new(RATE, 2);
    analyzer.push(&stereo(&sine(440.0, 0.5, 30.0, RATE)), secs(100.0));

    assert!(analyzer.frame_at(secs(100.5)).is_none());
    assert!(analyzer.frame_at(secs(129.5)).is_some());
    assert!(analyzer.frame_at(secs(129.0)).is_some());
}

#[test]
fn tempo_and_beats_end_when_the_music_does_and_clear_and_reconfigure_start_afresh() {
    let mut signal = with_bursts(12.0, RATE, &beat_times(120.0, 0.1, 12.0), &kick(RATE));
    signal.extend(silence(10.0, RATE));
    let run = play(&signal, 1, RATE, 512, 0.0, 0.15);
    assert!(run.near(11.5).tempo_bpm.is_some());
    let silent = run.near(21.5);
    assert_eq!(silent.tempo_bpm, None);
    assert!(silent.bands.iter().all(|&band| band == 0.0) && !silent.beat);

    let mut analyzer = run.analyzer;
    analyzer.clear();
    assert!(analyzer.frame_at(secs(21.5)).is_none());
    analyzer.reconfigure(48_000, 2);
    assert_eq!((analyzer.sample_rate(), analyzer.channels()), (48_000, 2));
    analyzer.push(&stereo(&sine(440.0, 0.5, 0.5, 48_000)), secs(0.0));
    assert!(analyzer.frame_at(secs(0.25)).is_some());
}

#[test]
fn bands_are_decibels_below_full_scale_mapped_from_minus_70_to_0() {
    // 1 kHz is where the weighting has no effect, and a tone on a bin centre loses nothing.
    for (amplitude, expected) in [
        (1.0_f32, 1.0),
        (0.1, 50.0 / 70.0),
        (0.01, 30.0 / 70.0),
        (0.0003, 0.0),
    ] {
        let hz = 10.0 * RATE as f32 / 2048.0 * 4.6;
        let mut analyzer = Analyzer::new(RATE, 1);
        analyzer.push(&sine(hz, amplitude, 0.5, RATE), secs(0.0));

        let frame = at(&analyzer, 0.25);

        let loudest = frame.bands[loudest_band(&frame)];
        assert!(
            (loudest - expected).abs() < 0.04,
            "amplitude {amplitude}: {loudest}, expected {expected}"
        );
    }
}

#[test]
fn rms_is_the_linear_level_of_both_channels() {
    let mut analyzer = Analyzer::new(RATE, 2);
    analyzer.push(
        &interleave(&sine(500.0, 1.0, 0.5, RATE), &silence(0.5, RATE)),
        secs(0.0),
    );
    let one_channel = at(&analyzer, 0.25).rms;
    // A full-scale sine in one channel and silence in the other: 0.707 / sqrt(2).
    assert!((one_channel - 0.5).abs() < 0.01, "{one_channel}");

    let square: Vec<f32> = (0..RATE as usize / 2)
        .map(|n| if (n / 40) % 2 == 0 { 1.0 } else { -1.0 })
        .collect();
    let mut analyzer = Analyzer::new(RATE, 1);
    analyzer.push(&square, secs(0.0));
    assert!((at(&analyzer, 0.25).rms - 1.0).abs() < 0.01);
}

#[test]
fn onset_is_quiet_for_a_steady_sound_and_high_for_a_sudden_one() {
    let signal = with_bursts(
        1.2,
        RATE,
        &[0.8],
        &noise(0.15, RATE, 31)
            .iter()
            .map(|v| v * 0.8)
            .collect::<Vec<_>>(),
    );
    let steady = noise(1.2, RATE, 33)
        .iter()
        .map(|v| v * 0.3)
        .collect::<Vec<f32>>();
    let sudden = play(&signal, 1, RATE, 512, 0.0, 0.1);
    let flat = play(&steady, 1, RATE, 512, 0.0, 0.1);

    let loudest_onset = |run: &Run, from: f64, to: f64| {
        run.frames
            .values()
            .filter(|frame| (from..to).contains(&(frame.t_ms as f64 / 1000.0)))
            .map(|frame| frame.onset)
            .fold(0.0_f32, f32::max)
    };
    assert!(loudest_onset(&sudden, 0.77, 0.87) > 0.8);
    assert!(loudest_onset(&sudden, 0.2, 0.7) < 0.02, "silence before");
    let hiss = loudest_onset(&flat, 0.3, 1.1);
    assert!(hiss < 0.3, "noise has no sudden change: {hiss}");
}
