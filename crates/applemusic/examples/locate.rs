//! Runs `locate_in_folder` against a saved snapshot of a library and a folder, and prints how
//! many songs were found and how. Only lists the folder; no file in it is opened.
//!
//! `cargo run --release -p unsub-applemusic --example locate --
//!   <snapshot.json> <folder> [unmatched-samples] [matched-samples-per-tier] [located.json]`

use std::collections::BTreeMap;
use std::env;
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use serde::Deserialize;
use unsub_applemusic::{Confidence, LocateStage, MusicLibraryItem, locate_in_folder};

#[derive(Deserialize)]
struct Snapshot {
    items: Vec<MusicLibraryItem>,
}

fn main() -> ExitCode {
    let mut args = env::args_os().skip(1);
    let (Some(snapshot), Some(folder)) = (args.next(), args.next()) else {
        eprintln!(
            "usage: locate <snapshot.json> <folder> [unmatched-samples] [matched-samples] [out.json]"
        );
        return ExitCode::FAILURE;
    };
    let mut count = || {
        args.next()
            .and_then(|count| count.to_str()?.parse().ok())
            .unwrap_or(0)
    };
    let (unmatched, matched) = (count(), count());
    let out = args.next().map(PathBuf::from);
    match run(
        PathBuf::from(snapshot),
        PathBuf::from(folder),
        unmatched,
        matched,
        out,
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run(
    snapshot: PathBuf,
    folder: PathBuf,
    unmatched_samples: usize,
    matched_samples: usize,
    out: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let snapshot: Snapshot = serde_json::from_reader(BufReader::new(File::open(snapshot)?))?;
    let started = Instant::now();
    let located = locate_in_folder(&snapshot.items, &folder, &mut |progress| {
        if progress.stage == LocateStage::Scanning && progress.done % 5_000 == 0 {
            eprintln!("listed {} files", progress.done);
        }
    })?;
    let elapsed = started.elapsed();

    let mut by_confidence: BTreeMap<String, usize> = BTreeMap::new();
    let mut dataless = 0;
    for found in located.matched.values() {
        let label = match found.confidence {
            Confidence::Exact => "exact",
            Confidence::Folder => "folder",
            Confidence::Fuzzy => "fuzzy",
        };
        *by_confidence.entry(label.to_owned()).or_default() += 1;
        dataless += usize::from(found.dataless);
    }
    println!("{} songs in the snapshot", snapshot.items.len());
    println!("matched {} in {elapsed:.1?}", located.matched.len());
    for (label, count) in by_confidence {
        println!("  {label:<8}{count}");
    }
    println!("  cloud-only (dataless): {dataless}");
    println!("unmatched {}", located.unmatched.len());
    if let Some(out) = out {
        serde_json::to_writer(File::create(out)?, &located)?;
    }

    let by_id: std::collections::HashMap<_, _> = snapshot
        .items
        .iter()
        .map(|item| (item.persistent_id.as_str(), item))
        .collect();
    for id in located.unmatched.iter().take(unmatched_samples) {
        let item = by_id[id.as_str()];
        println!(
            "  {} | {} | {} | #{:?} | {}",
            item.album_artist, item.artist, item.album, item.track_number, item.title
        );
    }
    for tier in [Confidence::Folder, Confidence::Fuzzy] {
        println!("{tier:?} matches, a sample");
        let in_tier = snapshot
            .items
            .iter()
            .filter_map(|item| Some((item, located.matched.get(&item.persistent_id)?)))
            .filter(|(_, found)| found.confidence == tier);
        // Evenly spread through the list rather than the first few.
        let all: Vec<_> = in_tier.collect();
        let step = (all.len() / matched_samples.max(1)).max(1);
        for (item, found) in all.into_iter().step_by(step).take(matched_samples) {
            let name = found.path.file_name().unwrap_or_default().to_string_lossy();
            println!("  {:?} #{:?} -> {name}", item.title, item.track_number);
        }
    }
    Ok(())
}
