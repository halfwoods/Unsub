//! Prints a summary of a music library: counts per ownership and media type, playlist counts, then
//! the first ten subscription items. Pass the path of an exported library XML, or no argument on macOS to read
//! the system library (which asks for "Media & Apple Music" access on first use).

use std::collections::BTreeMap;
use std::env;
use std::path::Path;
use std::process::ExitCode;

use antistreaming_applemusic::{MusicLibrary, Ownership, Result, read_xml_library};

fn main() -> ExitCode {
    let library = match env::args_os().nth(1) {
        Some(path) => read_xml_library(Path::new(&path)),
        None => read_system(),
    };
    match library {
        Ok(library) => {
            print_summary(&library);
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(target_os = "macos")]
fn read_system() -> Result<MusicLibrary> {
    antistreaming_applemusic::read_system_music_library()
}

#[cfg(not(target_os = "macos"))]
fn read_system() -> Result<MusicLibrary> {
    Err(antistreaming_applemusic::Error::Unavailable(
        "the system library is only readable on macOS; pass the path of an exported XML".into(),
    ))
}

fn print_summary(library: &MusicLibrary) {
    let items = &library.items;
    println!("{} items", items.len());

    println!("\nownership");
    print_counts(items.iter().map(|item| format!("{:?}", item.ownership)));

    println!("\nmedia");
    print_counts(items.iter().map(|item| format!("{:?}", item.media)));

    let playlists = &library.playlists;
    let top_level = playlists
        .iter()
        .filter(|playlist| playlist.parent_persistent_id.is_none())
        .count();
    let entries: usize = playlists
        .iter()
        .map(|playlist| playlist.item_ids.len())
        .sum();
    println!(
        "\n{} playlists ({top_level} at the top level, {entries} song entries)",
        playlists.len()
    );
    print_counts(
        playlists
            .iter()
            .map(|playlist| format!("{:?}", playlist.kind)),
    );

    println!("\nfirst 10 subscription items");
    let subscription = items
        .iter()
        .filter(|item| item.ownership == Ownership::Subscription);
    for item in subscription.take(10) {
        println!("  {} - {} ({})", item.artist, item.title, item.album);
    }
}

fn print_counts(labels: impl Iterator<Item = String>) {
    let mut counts = BTreeMap::<String, usize>::new();
    for label in labels {
        *counts.entry(label).or_default() += 1;
    }
    for (label, count) in counts {
        println!("  {label:<18}{count}");
    }
}
