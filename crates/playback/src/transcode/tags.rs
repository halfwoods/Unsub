//! Carrying tags and cover art across a conversion, and writing the gapless marker iTunes reads.

use std::path::Path;

use lofty::config::{ParseOptions, WriteOptions};
use lofty::file::{FileType, TaggedFileExt};
use lofty::mp4::{Atom, AtomData, AtomIdent, Ilst};
use lofty::probe::Probe;
use lofty::tag::{Tag, TagExt};

use crate::Result;

/// What iTunes and iPods use to trim an AAC file's encoder delay and padding.
pub struct Smpb {
    /// Samples of encoder delay at the start.
    pub priming: u64,
    /// Padding samples at the end.
    pub padding: u64,
    /// Samples of real audio between them.
    pub samples: u64,
}

impl Smpb {
    /// Apple's layout: eight hex fields after a leading space.
    fn render(&self) -> String {
        format!(
            " 00000000 {:08X} {:08X} {:016X} 00000000 00000000 00000000 00000000 00000000 00000000 00000000 00000000",
            self.priming, self.padding, self.samples
        )
    }
}

/// The tags of `path` with their cover art, or `None` if it has none or lofty cannot read the
/// format (a file without readable tags simply has nothing to carry over).
pub fn read_tag(path: &Path) -> Option<Tag> {
    let tagged = Probe::open(path)
        .ok()?
        .options(ParseOptions::new())
        .read()
        .ok()?;
    tagged.primary_tag().or_else(|| tagged.first_tag()).cloned()
}

/// Writes `tag` into `output` in its own format's native tag type, and `smpb` if it is an m4a.
pub fn write_tags(output: &Path, tag: Option<Tag>, smpb: Option<&Smpb>) -> Result<()> {
    let file_type = Probe::open(output)?.read()?.file_type();
    let tag_type = file_type.primary_tag_type();

    if file_type == FileType::Mp4
        && let Some(smpb) = smpb
    {
        let mut tag = tag.unwrap_or_else(|| Tag::new(tag_type));
        tag.re_map(tag_type);
        let mut ilst = Ilst::from(tag);
        ilst.insert(Atom::new(
            AtomIdent::Freeform {
                mean: "com.apple.iTunes".into(),
                name: "iTunSMPB".into(),
            },
            AtomData::UTF8(smpb.render()),
        ));
        ilst.save_to_path(output, WriteOptions::default())?;
        return Ok(());
    }
    if let Some(mut tag) = tag {
        tag.re_map(tag_type);
        tag.save_to_path(output, WriteOptions::default())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_marker_matches_apples_layout() {
        let smpb = Smpb {
            priming: 2112,
            padding: 758,
            samples: 43_210,
        };
        assert_eq!(
            smpb.render(),
            " 00000000 00000840 000002F6 000000000000A8CA 00000000 00000000 00000000 00000000 00000000 00000000 00000000 00000000"
        );
    }
}
