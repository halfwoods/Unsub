use crate::device::Ipod;
use crate::error::Result;

/// Unmounts the volume and powers the disk down so the iPod can be unplugged safely.
pub fn eject(ipod: &Ipod) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        diskutil_eject(&ipod.mount_path)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = ipod;
        Err(crate::error::Error::Unsupported(
            "ejecting is only implemented on macOS",
        ))
    }
}

#[cfg(target_os = "macos")]
fn diskutil_eject(mount: &std::path::Path) -> Result<()> {
    use std::process::Command;

    use crate::error::Error;

    let eject_error = |message: String| Error::Eject {
        mount: mount.to_path_buf(),
        message,
    };
    let output = Command::new("diskutil")
        .arg("eject")
        .arg(mount)
        .output()
        .map_err(|source| eject_error(source.to_string()))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let message = if stderr.trim().is_empty() {
        stdout
    } else {
        stderr
    };
    Err(eject_error(message.trim().to_owned()))
}
