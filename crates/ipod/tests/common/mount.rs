use std::fs;
use std::path::PathBuf;

use tempfile::TempDir;

/// A directory shaped like a mounted iPod: `<volume>/iPod_Control/{iTunes,Device,Music/F00}`.
pub struct FakeIpod {
    _root: TempDir,
    pub mount: PathBuf,
}

impl FakeIpod {
    pub fn new(volume: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let mount = root.path().join(volume);
        for dir in ["iTunes", "Device", "Music/F00"] {
            fs::create_dir_all(mount.join("iPod_Control").join(dir)).unwrap();
        }
        Self { _root: root, mount }
    }

    fn itunes(&self, file: &str) -> PathBuf {
        self.mount.join("iPod_Control").join("iTunes").join(file)
    }

    pub fn write_itunesdb(&self, bytes: &[u8]) {
        fs::write(self.itunes("iTunesDB"), bytes).unwrap();
    }

    pub fn write_itunescdb(&self, bytes: &[u8]) {
        fs::write(self.itunes("iTunesCDB"), bytes).unwrap();
    }

    pub fn write_play_counts(&self, bytes: &[u8]) {
        fs::write(self.itunes("Play Counts"), bytes).unwrap();
    }

    pub fn write_sysinfo(&self, model_num_str: &str, firewire_guid: &str) {
        let text = format!(
            "BoardHwName: Hades\nModelNumStr: {model_num_str}\nFirewireGuid: {firewire_guid}\nnot a key value line\n"
        );
        fs::write(self.mount.join("iPod_Control/Device/SysInfo"), text).unwrap();
    }

    /// A SysInfoExtended that lists its own album-art format (RGB565, little endian), the way a
    /// real iPod's does, and says it supports sparse artwork.
    pub fn write_sysinfo_extended_with_album_art(
        &self,
        firewire_guid: &str,
        db_version: i64,
        (format_id, width, height): (u32, u32, u32),
        row_bytes_alignment: u32,
    ) {
        let plist = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>FireWireGUID</key>
	<string>{firewire_guid}</string>
	<key>DBVersion</key>
	<integer>{db_version}</integer>
	<key>SupportsSparseArtwork</key>
	<true/>
	<key>AlbumArt</key>
	<array>
		<dict>
			<key>FormatId</key>
			<integer>{format_id}</integer>
			<key>RenderWidth</key>
			<integer>{width}</integer>
			<key>RenderHeight</key>
			<integer>{height}</integer>
			<key>PixelFormat</key>
			<string>4C353635</string>
			<key>RowBytesAlignment</key>
			<integer>{row_bytes_alignment}</integer>
			<key>BackColor</key>
			<string>FFFFFFFF</string>
		</dict>
	</array>
</dict>
</plist>
"#
        );
        fs::write(
            self.mount.join("iPod_Control/Device/SysInfoExtended"),
            plist,
        )
        .unwrap();
    }

    pub fn write_sysinfo_extended(&self, firewire_guid: &str, serial: &str, db_version: i64) {
        let plist = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>FamilyID</key>
	<integer>11</integer>
	<key>FireWireGUID</key>
	<string>{firewire_guid}</string>
	<key>SerialNumber</key>
	<string>{serial}</string>
	<key>DBVersion</key>
	<integer>{db_version}</integer>
	<key>PodcastsSupported</key>
	<true/>
	<key>MaxTransferSpeed</key>
	<integer>480000000</integer>
	<key>ImageSpecifications</key>
	<array>
		<dict>
			<key>FormatId</key>
			<integer>1055</integer>
			<key>GammaAdjustment</key>
			<real>1.0</real>
			<key>Interlaced</key>
			<false/>
			<key>PixelFormat</key>
			<string>4C353635</string>
		</dict>
	</array>
	<key>BuildID</key>
	<data>AQIDBA==</data>
	<key>RentalClockBias</key>
	<date>2010-06-15T12:00:00Z</date>
</dict>
</plist>
"#
        );
        fs::write(
            self.mount.join("iPod_Control/Device/SysInfoExtended"),
            plist,
        )
        .unwrap();
    }
}
