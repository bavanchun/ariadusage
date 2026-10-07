use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use rustix::fs::{FileType, OFlags};
use rustix::process::geteuid;

use super::error::BrowserError;

const MAX_DATABASE_COPY: u64 = 64 * 1024 * 1024;

pub(super) fn create_copy_dir(
    runtime_dir: Option<&Path>,
) -> Result<crate::PrivateTempDir, BrowserError> {
    let runtime_dir = runtime_dir.ok_or(BrowserError::Unsupported)?;
    if !is_trusted_tmpfs(runtime_dir) {
        return Err(BrowserError::Unsupported);
    }
    crate::private_tempdir::create_in(runtime_dir).map_err(|_| BrowserError::Unsupported)
}

pub(super) fn sweep_stale(runtime_dir: Option<&Path>) {
    if let Some(runtime_dir) = runtime_dir.filter(|path| is_trusted_tmpfs(path)) {
        crate::private_tempdir::sweep_stale_in(runtime_dir);
    }
}

pub(super) fn is_trusted_tmpfs(path: &Path) -> bool {
    let Ok(stat) = rustix::fs::stat(path) else {
        return false;
    };
    if FileType::from_raw_mode(stat.st_mode) != FileType::Directory
        || stat.st_uid != geteuid().as_raw()
        || stat.st_mode & 0o777 != 0o700
    {
        return false;
    }
    rustix::fs::statfs(path)
        .map(|fs| fs.f_type as u64 == crate::paths::TMPFS_MAGIC)
        .unwrap_or(false)
}

pub(super) fn copy_database(source: &Path, target_dir: &Path) -> Result<PathBuf, BrowserError> {
    let name = source.file_name().ok_or(BrowserError::Io)?;
    let target = target_dir.join(name);
    let total = copy_one(source, &target, 0)?;

    let wal = append_suffix(source, "-wal")?;
    let wal_target = append_suffix(&target, "-wal")?;
    let _ = copy_optional(&wal, &wal_target, total)?;
    Ok(target)
}

pub(super) fn read_owned_file(path: &Path, max_size: usize) -> Result<Vec<u8>, BrowserError> {
    read_optional_owned_file(path, max_size)?.ok_or(BrowserError::Io)
}

pub(super) fn read_optional_owned_file(
    path: &Path,
    max_size: usize,
) -> Result<Option<Vec<u8>>, BrowserError> {
    let file = match open_owned_file(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(BrowserError::Io),
    };
    let limit = u64::try_from(max_size)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    let mut bytes = Vec::with_capacity(max_size.min(64 * 1024));
    file.take(limit)
        .read_to_end(&mut bytes)
        .map_err(|_| BrowserError::Io)?;
    if bytes.len() > max_size {
        return Err(BrowserError::Io);
    }
    Ok(Some(bytes))
}

fn copy_one(source: &Path, target: &Path, already_copied: u64) -> Result<u64, BrowserError> {
    let Some(total) = copy_optional(source, target, already_copied)? else {
        return Err(BrowserError::NoBrowserSession);
    };
    Ok(total)
}

fn copy_optional(
    source: &Path,
    target: &Path,
    already_copied: u64,
) -> Result<Option<u64>, BrowserError> {
    let mut input = match open_owned_file(source) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(BrowserError::Io),
    };
    let remaining = MAX_DATABASE_COPY.saturating_sub(already_copied);
    if input.metadata().map_err(|_| BrowserError::Io)?.len() > remaining {
        return Err(BrowserError::Io);
    }
    let copied = {
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(target)
            .map_err(|_| BrowserError::Io)?;
        let mut limited = std::io::Read::take(&mut input, remaining.saturating_add(1));
        let copied = std::io::copy(&mut limited, &mut output).map_err(|_| BrowserError::Io)?;
        output.flush().map_err(|_| BrowserError::Io)?;
        copied
    };
    if copied > remaining {
        let _ = std::fs::remove_file(target);
        return Err(BrowserError::Io);
    }
    Ok(Some(already_copied + copied))
}

fn open_owned_file(path: &Path) -> Result<File, std::io::Error> {
    let fd = rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOCTTY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        rustix::fs::Mode::empty(),
    )
    .map_err(std::io::Error::from)?;
    let stat = rustix::fs::fstat(&fd).map_err(std::io::Error::from)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
        || stat.st_uid != geteuid().as_raw()
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "browser source rejected",
        ));
    }
    Ok(File::from(fd))
}

fn append_suffix(path: &Path, suffix: &str) -> Result<PathBuf, BrowserError> {
    let mut name = path.file_name().ok_or(BrowserError::Io)?.to_os_string();
    name.push(suffix);
    Ok(path.with_file_name(name))
}
