//! Beats found in music that looks like music: drums over a pad and a bass line, mastered hard,
//! and the other end of it, a quiet ambient passage with no pulse. `analysis.rs` shows that
//! clean clicks and kicks are found; this is whether they still are when a bass line sits on
//! them, whether drums that are not kicks give beats at all, and whether a swell gives none.
//!
//! Before the beat detector listened to the whole spectrum for sudden jumps, it listened to the
//! bass alone and wanted a 7 dB rise: it found none of the 48 kicks in the first piece, and none
//! of the snares in the third.

mod support;

use support::*;

const SECONDS: f32 = 24.0;
/// A beat counts for a hit this close to it.
const TOLERANCE: f64 = 0.035;
/// Beats and hits before this are the detector settling in.
const SETTLED: f32 = 3.0;

struct Piece {
    signal: Vec<f32>,
    /// What a listener would tap along to.
    hits: Vec<f32>,
    /// Other sounds that may fairly be answered with a beat, though they are not the pulse.
    ornaments: Vec<f32>,
}

impl Piece {
    /// The beats found, and how well they match the hits: the share of hits caught, and the
    /// beats that are near neither a hit nor an ornament.
    fn heard(&self) -> (Vec<f64>, f32, usize) {
        let run = play(&self.signal, 1, RATE, 512, 0.0, 0.15);
        let beats = beat_seconds(&run);
        let (recall, _) = score(&beats, &self.hits, SETTLED, TOLERANCE);
        let allowed: Vec<f32> = self.hits.iter().chain(&self.ornaments).copied().collect();
        let (_, stray) = score(&beats, &allowed, SETTLED, TOLERANCE);
        (beats, recall, stray)
    }
}

fn drums(signal: &mut [f32], times: &[f32], sound: &[f32], level: f32) {
    for &time in times {
        add(signal, RATE, time, sound, level);
    }
}

/// Kicks on the beat, hats between, a pad and a bass line, mastered hard.
fn four_on_the_floor() -> Piece {
    let beats = beat_times(124.0, 0.3, SECONDS - 0.5);
    let offbeats: Vec<f32> = beats.iter().map(|t| t + 60.0 / 124.0 / 2.0).collect();
    let mut signal: Vec<f32> = pad(SECONDS, RATE, 6.0).iter().map(|v| v * 0.4).collect();
    let notes = [55.0, 55.0, 65.4, 49.0];
    add(
        &mut signal,
        RATE,
        0.0,
        &bass_line(SECONDS, RATE, &notes, 60.0 / 124.0 / 2.0),
        0.25,
    );
    drums(&mut signal, &beats, &drum_kick(RATE), 0.7);
    drums(&mut signal, &offbeats, &hat(RATE), 0.5);
    master(&mut signal, 1.7);
    Piece {
        signal,
        hits: beats,
        ornaments: offbeats,
    }
}

/// A kick every beat, over a pad and two bass notes that never stop, with `level` the kick's
/// share of what the bass is.
fn kick_in_a_wall_of_sound(level: f32) -> Piece {
    let beats = beat_times(92.0, 0.4, SECONDS - 0.5);
    let mut signal = pad(SECONDS, RATE, 6.0);
    add(&mut signal, RATE, 0.0, &sine(55.0, 0.3, SECONDS, RATE), 1.0);
    add(
        &mut signal,
        RATE,
        0.0,
        &sine(82.4, 0.15, SECONDS, RATE),
        1.0,
    );
    drums(&mut signal, &beats, &drum_kick(RATE), level);
    master(&mut signal, 1.4);
    Piece {
        signal,
        hits: beats,
        ornaments: vec![],
    }
}

/// No kick at all: a snare on two and four, hats on the eighths, a pad and a steady bass.
fn backbeat_without_a_kick() -> Piece {
    let bars = beat_times(100.0, 0.3, SECONDS - 0.5);
    let snares: Vec<f32> = bars.iter().skip(1).step_by(2).copied().collect();
    let eighths: Vec<f32> = bars.iter().flat_map(|t| [*t, t + 0.3]).collect();
    let mut signal = pad(SECONDS, RATE, 6.0);
    add(
        &mut signal,
        RATE,
        0.0,
        &sine(73.4, 0.25, SECONDS, RATE),
        1.0,
    );
    drums(&mut signal, &snares, &snare(RATE), 0.6);
    drums(&mut signal, &eighths, &hat(RATE), 0.5);
    master(&mut signal, 1.3);
    Piece {
        signal,
        hits: snares,
        ornaments: eighths,
    }
}

/// A soft kick on one and three at 70 BPM, a pad, and plucked notes that follow no pulse.
fn sparse_ballad() -> Piece {
    let beats: Vec<f32> = beat_times(70.0, 0.5, SECONDS - 0.5)
        .into_iter()
        .step_by(2)
        .collect();
    let plucks = vec![1.9, 4.6, 5.2, 8.8, 11.3, 12.1, 15.7, 18.4, 19.0, 22.2];
    let mut signal = pad(SECONDS, RATE, 6.0);
    for &time in &plucks {
        add(
            &mut signal,
            RATE,
            time,
            &pluck(330.0 + time * 7.0, 0.8, RATE),
            0.35,
        );
    }
    drums(&mut signal, &beats, &drum_kick(RATE), 0.35);
    Piece {
        signal,
        hits: beats,
        ornaments: plucks,
    }
}

/// Swells and a few notes. There is no pulse; nothing should flash on the swells.
fn ambient() -> Piece {
    let plucks = vec![2.4, 7.9, 8.6, 14.1, 20.3];
    let mut signal = pad(SECONDS, RATE, 8.0);
    for (n, &time) in plucks.iter().enumerate() {
        add(
            &mut signal,
            RATE,
            time,
            &pluck(220.0 * (1.0 + n as f32 * 0.25), 1.2, RATE),
            0.3,
        );
    }
    Piece {
        signal,
        hits: vec![],
        ornaments: plucks,
    }
}

/// A clipped sawtooth power chord, palm-muted: gone as fast as it came.
fn chug(roots: [f32; 2], length: f32) -> Vec<f32> {
    (0..(length * RATE as f32) as usize)
        .map(|n| {
            let t = n as f32 / RATE as f32;
            let saw = |hz: f32| 2.0 * ((t * hz) % 1.0) - 1.0;
            let raw: f32 = roots.iter().map(|&hz| saw(hz) + 0.5 * saw(2.0 * hz)).sum();
            (3.0 * raw).tanh() * (t / 0.004).min(1.0) * (-t * 14.0).exp()
        })
        .collect()
}

/// Kick on one and three, snare on two and four, hats and distorted guitar chugging on the
/// eighths: everything on the grid is a hit of some kind.
fn rock() -> Piece {
    let beat = 60.0 / 140.0;
    let beats = beat_times(140.0, 0.3, SECONDS - 0.5);
    let kicks: Vec<f32> = beats.iter().step_by(2).copied().collect();
    let snares: Vec<f32> = beats.iter().skip(1).step_by(2).copied().collect();
    let eighths: Vec<f32> = beats.iter().flat_map(|t| [*t, t + beat / 2.0]).collect();
    let mut signal = vec![0.0; (SECONDS * RATE as f32) as usize];
    for (n, &time) in eighths.iter().enumerate() {
        let root = [82.4, 82.4, 110.0, 98.0][(n / 16) % 4];
        add(
            &mut signal,
            RATE,
            time,
            &chug([root, root * 1.5], beat / 2.0),
            0.35,
        );
    }
    drums(&mut signal, &kicks, &drum_kick(RATE), 0.7);
    drums(&mut signal, &snares, &snare(RATE), 0.6);
    drums(&mut signal, &eighths, &hat(RATE), 0.35);
    master(&mut signal, 1.5);
    Piece {
        signal,
        hits: beats,
        ornaments: eighths,
    }
}

#[test]
fn kicks_over_a_mastered_mix_are_beats() {
    let piece = four_on_the_floor();
    let (beats, recall, stray) = piece.heard();
    assert!(recall >= 0.9, "{:.0}% of the kicks", recall * 100.0);
    assert!(stray <= 2, "{stray} beats on nothing");
    // The hats get beats too, but the picture is not asked to flash on every sixteenth.
    assert!(
        beats.len() <= 2 * piece.hits.len() + 4,
        "{} beats",
        beats.len()
    );
}

#[test]
fn a_kick_that_stands_out_of_a_wall_of_sound_is_a_beat() {
    let (_, recall, stray) = kick_in_a_wall_of_sound(0.7).heard();
    assert!(recall >= 0.9, "{:.0}% of the kicks", recall * 100.0);
    assert_eq!(stray, 0);
}

#[test]
fn a_kick_buried_in_the_wall_is_missed_rather_than_guessed() {
    let (_, _, stray) = kick_in_a_wall_of_sound(0.2).heard();
    assert_eq!(stray, 0);
}

#[test]
fn snares_and_hats_without_a_kick_are_beats() {
    let (_, recall, stray) = backbeat_without_a_kick().heard();
    assert!(recall >= 0.9, "{:.0}% of the snares", recall * 100.0);
    assert!(stray <= 2, "{stray} beats on nothing");
}

#[test]
fn drums_under_distorted_guitars_are_beats() {
    let (_, recall, stray) = rock().heard();
    assert!(
        recall >= 0.9,
        "{:.0}% of the kicks and snares",
        recall * 100.0
    );
    assert_eq!(stray, 0, "a beat off the eighth-note grid");
}

#[test]
fn soft_kicks_between_notes_are_beats() {
    let (_, recall, stray) = sparse_ballad().heard();
    assert!(recall >= 0.85, "{:.0}% of the kicks", recall * 100.0);
    assert!(stray <= 1, "{stray} beats on nothing");
}

#[test]
fn a_kick_at_a_whisper_is_a_beat() {
    let beats = beat_times(110.0, 0.3, SECONDS - 0.5);
    let mut signal: Vec<f32> = noise(SECONDS, RATE, 5).iter().map(|v| v * 0.001).collect();
    drums(&mut signal, &beats, &drum_kick(RATE), 0.04);
    let piece = Piece {
        signal,
        hits: beats,
        ornaments: vec![],
    };

    let (_, recall, stray) = piece.heard();
    assert!(recall >= 0.95, "{:.0}% of the kicks", recall * 100.0);
    assert_eq!(stray, 0);
}

#[test]
fn the_tempo_is_that_of_the_kicks_in_a_mix_and_there_is_none_in_ambience() {
    let last_tempo = |piece: &Piece| {
        let run = play(&piece.signal, 1, RATE, 512, 0.0, 0.15);
        run.frames.values().last().and_then(|frame| frame.tempo_bpm)
    };
    let house = last_tempo(&four_on_the_floor()).expect("a tempo");
    assert!((house - 124.0).abs() < 2.0, "{house} BPM");
    let wall = last_tempo(&kick_in_a_wall_of_sound(0.7)).expect("a tempo");
    assert!((wall - 92.0).abs() < 2.0, "{wall} BPM");
    assert_eq!(last_tempo(&ambient()), None);
}

#[test]
fn ambient_swells_are_not_beats() {
    let (beats, _, stray) = ambient().heard();
    assert_eq!(stray, 0, "a beat on a swell");
    assert!(
        beats.len() <= 3,
        "{} beats in 24 s of ambience",
        beats.len()
    );
}

#[test]
fn the_onset_stands_out_on_the_hits() {
    let piece = backbeat_without_a_kick();
    let run = play(&piece.signal, 1, RATE, 512, 0.0, 0.15);
    let mut onsets: Vec<f32> = run.frames.values().map(|frame| frame.onset).collect();
    onsets.sort_by(f32::total_cmp);
    let usual = onsets[onsets.len() / 2];
    let snares: Vec<f32> = piece
        .hits
        .iter()
        .filter(|&&hit| hit > SETTLED)
        .map(|&hit| run.near(f64::from(hit) + 0.01).onset)
        .collect();
    let on_hits = snares.iter().sum::<f32>() / snares.len() as f32;
    assert!(
        on_hits > 8.0 * usual && on_hits > 0.4,
        "{on_hits} on the snares, {usual} usually"
    );
}
