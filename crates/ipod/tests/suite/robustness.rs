use antistreaming_ipod::{Error, Ipod, read_database};

use crate::common::mount::FakeIpod;
use crate::{classic_7g, opened, sample_database};

fn corrupt(bytes: &[u8]) -> Result<antistreaming_ipod::Database, Error> {
    let fake = FakeIpod::new("IPOD");
    fake.write_itunesdb(bytes);
    read_database(&opened(&fake))
}

fn is_corrupt(result: Result<antistreaming_ipod::Database, Error>) -> bool {
    matches!(result, Err(Error::Corrupt { .. }))
}

/// Deterministic xorshift so a failure reproduces without a rand dependency.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, bound: usize) -> usize {
        usize::try_from(self.next() % u64::try_from(bound).unwrap()).unwrap()
    }
}

fn read_bytes(
    ipod: &Ipod,
    fake: &FakeIpod,
    bytes: &[u8],
) -> Result<antistreaming_ipod::Database, Error> {
    fake.write_itunesdb(bytes);
    read_database(ipod)
}

#[test]
fn garbage_and_empty_files_are_corrupt() {
    assert!(is_corrupt(corrupt(b"")));
    assert!(is_corrupt(corrupt(b"mhbd")));
    assert!(is_corrupt(corrupt(&[0xff; 64])));
    assert!(is_corrupt(corrupt(&[0; 4096])));
    assert!(is_corrupt(corrupt(
        b"definitely not an iTunesDB, just some text of a decent length"
    )));
}

#[test]
fn a_big_endian_database_is_rejected_with_a_clear_reason() {
    let mut bytes = sample_database().build();
    bytes[..4].copy_from_slice(b"dbhm");

    let Err(Error::Corrupt { reason, .. }) = corrupt(&bytes) else {
        panic!("expected Corrupt")
    };

    assert!(reason.contains("big-endian"), "{reason}");
}

#[test]
fn every_truncation_of_a_valid_database_is_corrupt() {
    let bytes = sample_database().build();
    let fake = classic_7g(&sample_database());
    let ipod = opened(&fake);

    for cut in 0..bytes.len() {
        let result = read_bytes(&ipod, &fake, &bytes[..cut]);

        assert!(matches!(result, Err(Error::Corrupt { .. })), "cut at {cut}");
    }
    assert!(read_bytes(&ipod, &fake, &bytes).is_ok());
}

#[test]
fn truncations_with_a_consistent_outer_length_never_panic() {
    let bytes = sample_database().build();
    let fake = classic_7g(&sample_database());
    let ipod = opened(&fake);

    for cut in 12..bytes.len() {
        let mut truncated = bytes[..cut].to_vec();
        truncated[8..12].copy_from_slice(&u32::try_from(cut).unwrap().to_le_bytes());
        let _ = read_bytes(&ipod, &fake, &truncated);
    }
}

#[test]
fn truncations_report_the_corrupt_offset() {
    let bytes = sample_database().build();

    let Err(Error::Corrupt { offset, .. }) = corrupt(&bytes[..bytes.len() / 2]) else {
        panic!("expected Corrupt")
    };

    assert!(offset < bytes.len());
}

#[test]
fn random_byte_damage_never_panics() {
    let bytes = sample_database().build();
    let fake = classic_7g(&sample_database());
    let ipod = opened(&fake);
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);

    for _ in 0..3000 {
        let mut damaged = bytes.clone();
        for _ in 0..=rng.below(8) {
            let at = rng.below(damaged.len());
            damaged[at] = u8::try_from(rng.next() & 0xff).unwrap();
        }
        let _ = read_bytes(&ipod, &fake, &damaged);
    }
}

#[test]
fn damaged_length_fields_never_panic() {
    let bytes = sample_database().build();
    let fake = classic_7g(&sample_database());
    let ipod = opened(&fake);
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    const TAGS: [&[u8]; 7] = [
        b"mhsd", b"mhlt", b"mhit", b"mhlp", b"mhyp", b"mhip", b"mhod",
    ];
    let tags: Vec<usize> = bytes
        .windows(4)
        .enumerate()
        .filter(|&(_, window)| TAGS.contains(&window))
        .map(|(at, _)| at)
        .collect();
    let interesting = [0u32, 1, 11, 12, 0x7fff_ffff, 0xffff_ffff, 0x1_0000];

    for _ in 0..3000 {
        let mut damaged = bytes.clone();
        let tag = tags[rng.below(tags.len())];
        let field = tag + 4 + 4 * rng.below(2);
        let value = interesting[rng.below(interesting.len())];
        damaged[field..field + 4].copy_from_slice(&value.to_le_bytes());
        let _ = read_bytes(&ipod, &fake, &damaged);
    }
}

#[test]
fn a_zero_length_chunk_cannot_hang_the_reader() {
    let mut bytes = sample_database().build();
    // Zero the first mhsd's total length.
    bytes[0xbc + 8..0xbc + 12].copy_from_slice(&0u32.to_le_bytes());

    assert!(is_corrupt(corrupt(&bytes)));
}
