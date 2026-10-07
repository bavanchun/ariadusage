use std::io::Read;
use std::path::{Path, PathBuf};

use zeroize::Zeroizing;

pub const MAX_ENVIRONMENT_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProcessIdentity {
    pub pid: i32,
    pub start_ticks: u64,
}

pub fn same_uid_processes(root: &Path, uid: u32) -> Vec<ProcessIdentity> {
    #[cfg(target_os = "linux")]
    {
        use procfs::process::all_processes_with_root;

        let Ok(processes) = all_processes_with_root(root) else {
            return Vec::new();
        };
        let current_pid = std::process::id() as i32;
        let mut identities = processes
            .filter_map(Result::ok)
            .filter(|process| process.pid != current_pid)
            .filter_map(|process| {
                let status = process.status().ok()?;
                if status.ruid != uid {
                    return None;
                }
                let stat = process.stat().ok()?;
                Some(ProcessIdentity {
                    pid: process.pid,
                    start_ticks: stat.starttime,
                })
            })
            .collect::<Vec<_>>();
        identities.sort_by_key(|identity| identity.pid);
        identities
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (root, uid);
        Vec::new()
    }
}

pub fn fd_targets(root: &Path, pid: i32) -> Vec<PathBuf> {
    if pid <= 0 {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(root.join(pid.to_string()).join("fd")) else {
        return Vec::new();
    };
    let mut targets = entries
        .filter_map(Result::ok)
        .filter_map(|entry| std::fs::read_link(entry.path()).ok())
        .collect::<Vec<_>>();
    targets.sort();
    targets
}

pub fn process_identity(root: &Path, pid: i32) -> Option<ProcessIdentity> {
    if pid <= 0 {
        return None;
    }
    let stat = std::fs::read_to_string(root.join(pid.to_string()).join("stat")).ok()?;
    let fields = stat
        .get(stat.rfind(')')?.checked_add(1)?..)?
        .split_whitespace();
    let start_ticks = fields.clone().nth(19)?.parse().ok()?;
    Some(ProcessIdentity { pid, start_ticks })
}

pub fn process_group(root: &Path, pid: i32) -> Option<i32> {
    if pid <= 0 {
        return None;
    }
    let stat = std::fs::read_to_string(root.join(pid.to_string()).join("stat")).ok()?;
    let suffix_start = stat.rfind(')')?.checked_add(1)?;
    stat.get(suffix_start..)?
        .split_whitespace()
        .nth(2)?
        .parse()
        .ok()
}

pub fn process_uid(root: &Path, pid: i32) -> Option<u32> {
    if pid <= 0 {
        return None;
    }
    let status = std::fs::read_to_string(root.join(pid.to_string()).join("status")).ok()?;
    status.lines().find_map(|line| {
        let mut values = line.strip_prefix("Uid:")?.split_whitespace();
        values.next()?.parse().ok()
    })
}

pub fn read_marker_environment(root: &Path, pid: i32, key: &str) -> Option<Zeroizing<Vec<u8>>> {
    if pid <= 0 {
        return None;
    }
    let path = root.join(pid.to_string()).join("environ");
    let file = std::fs::File::open(path).ok()?;
    let mut bytes = Zeroizing::new(Vec::with_capacity(4096));
    file.take((MAX_ENVIRONMENT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .ok()?;
    parse_marker_environment(&bytes, key)
}

pub fn parse_marker_environment(bytes: &[u8], key: &str) -> Option<Zeroizing<Vec<u8>>> {
    if bytes.len() > MAX_ENVIRONMENT_BYTES || bytes.is_empty() || bytes.last() != Some(&0) {
        return None;
    }
    let key = key.as_bytes();
    let mut marker: Option<Zeroizing<Vec<u8>>> = None;
    for record in bytes[..bytes.len() - 1].split(|byte| *byte == 0) {
        if record.is_empty() {
            return None;
        }
        let separator = record.iter().position(|byte| *byte == b'=')?;
        if separator == 0 {
            return None;
        }
        if &record[..separator] != key {
            continue;
        }
        let value = &record[separator + 1..];
        if let Some(current) = marker.as_ref()
            && current.as_slice() != value
        {
            return None;
        }
        if marker.is_none() {
            marker = Some(Zeroizing::new(value.to_vec()));
        }
    }
    marker
}
