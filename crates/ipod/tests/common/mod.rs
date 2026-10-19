//! Test-only writer for iTunesDB files, shaped like what iTunes 9/10 produced, plus fake iPod
//! mounts. It shares no code with the reader so a misreading cannot cancel out on both sides.

pub mod db;
pub mod mount;
