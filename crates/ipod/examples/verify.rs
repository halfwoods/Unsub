//! Checks that this crate can rewrite a real iPod's database losslessly, without writing anything.
//!
//! ```text
//! cargo run -p antistreaming-ipod --example verify -- /Volumes/IPOD
//! ```
//!
//! Safe to run on a real iPod: the device is only read.

use std::path::PathBuf;
use std::process::ExitCode;

use antistreaming_ipod::{ArtworkCheck, ChecksumCheck, RecordDifference, open, verify_roundtrip};

/// Differences listed per kind before the rest are summarised.
const SHOWN: usize = 10;

fn main() -> ExitCode {
    let Some(mount) = std::env::args_os().nth(1).map(PathBuf::from) else {
        eprintln!("usage: verify <iPod mount path>");
        return ExitCode::from(2);
    };
    let ipod = match open(&mount) {
        Ok(ipod) => ipod,
        Err(error) => {
            eprintln!("cannot open {}: {error}", mount.display());
            return ExitCode::from(2);
        }
    };
    println!(
        "{} ({}), database {:?}, sync {:?}",
        ipod.name,
        ipod.model
            .as_ref()
            .map_or("unidentified model", |model| model.name.as_str()),
        ipod.database,
        ipod.sync,
    );

    let report = match verify_roundtrip(&ipod) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("the database cannot be rewritten: {error}");
            return ExitCode::from(2);
        }
    };

    println!(
        "{} tracks, {} playlists; {} bytes read, {} bytes rewritten ({:+})",
        report.tracks,
        report.playlists,
        report.original_bytes,
        report.rewritten_bytes,
        report.rewritten_bytes as i64 - report.original_bytes as i64,
    );
    println!("checksum: {}", describe(report.checksum));
    match &report.artwork {
        ArtworkCheck::Absent => println!("artwork: no ArtworkDB"),
        ArtworkCheck::Unreadable(reason) => println!("artwork: ArtworkDB UNREADABLE: {reason}"),
        ArtworkCheck::Checked {
            images,
            thumbnails,
            missing_thumbnails,
            identical,
        } => println!(
            "artwork: {images} images, {thumbnails} thumbnails ({missing_thumbnails} outside their files); \
             rewriting reproduces it {}",
            if *identical { "exactly" } else { "DIFFERENTLY" },
        ),
    }
    println!("sections:");
    for section in &report.sections {
        println!(
            "  type {:>2}: {:>10} -> {:>10} bytes ({:+}) {}",
            section.kind,
            section.original_bytes,
            section.rewritten_bytes,
            section.rewritten_bytes as i64 - section.original_bytes as i64,
            if section.rebuilt { "rebuilt" } else { "copied" },
        );
    }
    println!("header words changed:");
    for change in &report.header_changes {
        println!(
            "  {} +{:#05x}: {} records, e.g. {:#010x} -> {:#010x}{}",
            change.record,
            change.offset,
            change.records_changed,
            change.example_before,
            change.example_after,
            if change.expected {
                ""
            } else {
                "  <-- unexpected"
            },
        );
    }
    println!("chunk counts that differ:");
    for count in &report.chunk_counts {
        println!(
            "  {}: {} -> {}",
            count.chunk, count.original, count.rewritten
        );
    }
    list("track differences", &report.track_differences);
    list("playlist differences", &report.playlist_differences);
    if !report.mhod_differences.is_empty() {
        println!(
            "records whose mhods changed: {}",
            report.mhod_differences.join(", ")
        );
    }

    if report.is_lossless() {
        println!("RESULT: lossless");
        ExitCode::SUCCESS
    } else {
        println!("RESULT: differences found; do not sync to this iPod until they are explained");
        ExitCode::FAILURE
    }
}

fn describe(check: ChecksumCheck) -> String {
    match check {
        ChecksumCheck::Verified => {
            "HASH58 stored by iTunes matches this crate's computation".to_owned()
        }
        ChecksumCheck::Mismatch => {
            "HASH58 stored by iTunes does NOT match this crate's computation".to_owned()
        }
        ChecksumCheck::Absent => "none stored".to_owned(),
        ChecksumCheck::OtherScheme(scheme) => {
            format!("scheme {scheme}, which this crate cannot write")
        }
        ChecksumCheck::NoFirewireGuid => {
            "HASH58 stored, but the iPod's FireWire GUID is unknown".to_owned()
        }
    }
}

fn list(heading: &str, differences: &[RecordDifference]) {
    println!("{heading}: {}", differences.len());
    for difference in differences.iter().take(SHOWN) {
        println!("  {} \"{}\"", difference.record, difference.name);
        for field in &difference.fields {
            println!("    {}: {} -> {}", field.field, field.before, field.after);
        }
    }
    if differences.len() > SHOWN {
        println!("  ... and {} more", differences.len() - SHOWN);
    }
}
