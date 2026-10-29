//! Opening a file without following a link at its name, for a read that must not go through one.

use std::fs;
use std::path::Path;

/// Opens a file without following a link at its name, so a target that became a link between the
/// clash check and this open is refused rather than read through. The handle is checked to be a
/// regular file before it is used: a FIFO that appeared at the name would otherwise wait for a
/// writer, or read whatever was written to it, and the check that saw a file is the wrong check for
/// it. `NONBLOCK` is what keeps the open of a FIFO from waiting.
pub(super) fn open_no_follow(path: &Path) -> std::io::Result<fs::File> {
    #[cfg(unix)]
    {
        use rustix::fs::{FileType, Mode, OFlags};
        let fd = rustix::fs::open(
            path,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        let file = fs::File::from(fd);
        let stat = rustix::fs::fstat(&file)?;
        if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
            return Err(not_regular(path));
        }
        Ok(file)
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_FLAG_OPEN_REPARSE_POINT: the name itself is opened, not what it points at.
        const OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        let file = fs::OpenOptions::new()
            .read(true)
            .custom_flags(OPEN_REPARSE_POINT)
            .open(path)?;
        if !file.metadata()?.file_type().is_file() {
            return Err(not_regular(path));
        }
        Ok(file)
    }
    #[cfg(not(any(unix, windows)))]
    {
        fs::File::open(path)
    }
}

/// The refusal for a name that is not a regular file.
fn not_regular(path: &Path) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        format!("{} is not a regular file", path.display()),
    )
}

/// Opens a file for setting its time and mode without following a link at its name. On Windows a
/// handle opened only for reading cannot set them, and a read-only file cannot be opened for
/// writing, so this asks for the two attribute rights alone.
pub(super) fn open_for_metadata(path: &Path) -> std::io::Result<fs::File> {
    #[cfg(unix)]
    {
        open_no_follow(path)
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_READ_ATTRIBUTES: u32 = 0x0080;
        const FILE_WRITE_ATTRIBUTES: u32 = 0x0100;
        const SYNCHRONIZE: u32 = 0x0010_0000;
        const OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        fs::OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES | FILE_WRITE_ATTRIBUTES | SYNCHRONIZE)
            .custom_flags(OPEN_REPARSE_POINT)
            .open(path)
    }
    #[cfg(not(any(unix, windows)))]
    {
        fs::File::open(path)
    }
}
