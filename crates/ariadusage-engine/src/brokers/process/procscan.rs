// Ported from CodexBar Sources/CodexBarCore/DarwinProcessEnumerator.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/PiProcessEnvironment.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Host/Process/ProcessOwnershipReaper.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::io::Read;
use std::path::{Path, PathBuf};
use std::{collections::BTreeMap, collections::BTreeSet, fs};

use zeroize::Zeroizing;

pub const MAX_ENVIRONMENT_BYTES: usize = 1024 * 1024;
#[cfg(unix)]
pub const PROCESS_MARKER_ENV: &str = "ARIADUSAGE_PROCESS_MARKER";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProcessIdentity {
    pub pid: i32,
    pub start_ticks: u64,
}

#[cfg(target_os = "macos")]
pub(crate) fn is_live_proc_root(root: &Path) -> bool {
    root == Path::new("/proc")
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
    #[cfg(target_os = "macos")]
    {
        if !is_live_proc_root(root) {
            return same_uid_processes_from_proc_fs(root, uid);
        }
        let pids = darwin_same_uid_pids(uid);
        let current_pid = std::process::id() as i32;
        let mut seen = BTreeSet::new();
        let mut identities = Vec::new();
        for pid in pids {
            if pid <= 0 || pid == current_pid || !seen.insert(pid) {
                continue;
            }
            let Some(info) = darwin_bsd_info(pid) else {
                continue;
            };
            if info.pbi_ruid != uid {
                continue;
            }
            identities.push(darwin_identity_from_bsd(pid, &info));
        }
        identities.sort_by_key(|identity| identity.pid);
        identities
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        same_uid_processes_from_proc_fs(root, uid)
    }
}

#[cfg(not(target_os = "linux"))]
fn same_uid_processes_from_proc_fs(root: &Path, uid: u32) -> Vec<ProcessIdentity> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let current_pid = std::process::id() as i32;
    let mut identities = entries
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().to_string_lossy().parse::<i32>().ok())
        .filter(|&pid| pid > 0 && pid != current_pid)
        .filter(|&pid| process_uid_from_proc_fs(root, pid) == Some(uid))
        .filter_map(|pid| process_identity_from_proc_fs(root, pid))
        .collect::<Vec<_>>();
    identities.sort_by_key(|identity| identity.pid);
    identities
}

pub fn fd_targets(root: &Path, pid: i32) -> Vec<PathBuf> {
    if pid <= 0 {
        return Vec::new();
    }
    #[cfg(target_os = "macos")]
    if is_live_proc_root(root) {
        return darwin_fd_targets(pid);
    }
    fd_targets_from_proc_fs(root, pid)
}

pub fn process_cwd(root: &Path, pid: i32) -> Option<PathBuf> {
    if pid <= 0 {
        return None;
    }
    #[cfg(target_os = "macos")]
    if is_live_proc_root(root) {
        return darwin_process_cwd(pid);
    }
    let before = proc_fs_owned_identity(root, pid)?;
    let cwd = std::fs::read_link(root.join(pid.to_string()).join("cwd")).ok()?;
    if proc_fs_owned_identity(root, pid) != Some(before) {
        return None;
    }
    Some(cwd)
}

fn fd_targets_from_proc_fs(root: &Path, pid: i32) -> Vec<PathBuf> {
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
    #[cfg(target_os = "macos")]
    if is_live_proc_root(root) {
        let info = darwin_bsd_info(pid)?;
        return Some(darwin_identity_from_bsd(pid, &info));
    }
    process_identity_from_proc_fs(root, pid)
}

fn process_identity_from_proc_fs(root: &Path, pid: i32) -> Option<ProcessIdentity> {
    let stat = std::fs::read_to_string(root.join(pid.to_string()).join("stat")).ok()?;
    let fields = stat
        .get(stat.rfind(')')?.checked_add(1)?..)?
        .split_whitespace();
    let start_ticks = fields.clone().nth(19)?.parse().ok()?;
    Some(ProcessIdentity { pid, start_ticks })
}

pub fn process_state(root: &Path, pid: i32) -> Option<char> {
    if pid <= 0 {
        return None;
    }
    #[cfg(target_os = "macos")]
    if is_live_proc_root(root) {
        if let Some(info) = darwin_bsd_info(pid) {
            return Some(darwin_status_char(info.pbi_status));
        }
        if let Some(short) = darwin_bsd_short_info(pid) {
            return Some(darwin_status_char(short.pbsi_status));
        }
        if darwin_is_zombie_child(pid) {
            return Some('Z');
        }
        return None;
    }
    let stat = std::fs::read_to_string(root.join(pid.to_string()).join("stat")).ok()?;
    stat.get(stat.rfind(')')?.checked_add(1)?..)?
        .split_whitespace()
        .next()?
        .chars()
        .next()
}

pub fn process_group(root: &Path, pid: i32) -> Option<i32> {
    if pid <= 0 {
        return None;
    }
    #[cfg(target_os = "macos")]
    if is_live_proc_root(root) {
        if let Some(info) = darwin_bsd_info(pid) {
            return i32::try_from(info.pbi_pgid).ok();
        }
        if let Some(short) = darwin_bsd_short_info(pid) {
            return i32::try_from(short.pbsi_pgid).ok();
        }
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

pub fn process_net_ns(root: &Path, pid: i32) -> Option<String> {
    if pid <= 0 {
        return None;
    }
    #[cfg(target_os = "macos")]
    if is_live_proc_root(root) {
        return darwin_bsd_info(pid).map(|_| "darwin-host".to_owned());
    }
    #[cfg(unix)]
    {
        std::fs::read_link(root.join(pid.to_string()).join("ns").join("net"))
            .ok()
            .map(|path| path.to_string_lossy().into_owned())
    }
    #[cfg(not(unix))]
    {
        let _ = root;
        None
    }
}

pub fn process_children(root: &Path, pid: i32) -> Vec<i32> {
    if pid <= 0 {
        return Vec::new();
    }
    #[cfg(target_os = "macos")]
    if is_live_proc_root(root) {
        return darwin_process_children(pid);
    }
    let task_root = root.join(pid.to_string()).join("task");
    let Ok(tasks) = fs::read_dir(task_root) else {
        return Vec::new();
    };
    let mut children = BTreeSet::new();
    for task in tasks.filter_map(Result::ok) {
        let Ok(task_id) = task.file_name().to_string_lossy().parse::<i32>() else {
            continue;
        };
        let Ok(contents) = fs::read_to_string(task.path().join("children")) else {
            continue;
        };
        children.extend(
            contents
                .split_whitespace()
                .filter_map(|value| value.parse::<i32>().ok())
                .filter(|child| *child > 0 && *child != task_id),
        );
    }
    children.into_iter().collect()
}

pub fn process_descendants(
    root: &Path,
    identity: ProcessIdentity,
    uid: u32,
) -> Vec<ProcessIdentity> {
    process_descendants_with_hook(root, identity, uid, || {})
}

fn process_descendants_with_hook(
    root: &Path,
    root_identity: ProcessIdentity,
    uid: u32,
    after_walk: impl FnOnce(),
) -> Vec<ProcessIdentity> {
    if !identity_is_current(root, root_identity, uid) {
        return Vec::new();
    }

    let mut pending = process_children(root, root_identity.pid);
    let mut seen = BTreeSet::new();
    let mut found = BTreeMap::new();
    while let Some(pid) = pending.pop() {
        if !seen.insert(pid) {
            continue;
        }
        let Some(child_identity) = process_identity(root, pid) else {
            continue;
        };
        if process_uid(root, pid) != Some(uid) {
            continue;
        }
        let children = process_children(root, pid);
        if !identity_is_current(root, child_identity, uid) {
            continue;
        }
        if !identity_is_current(root, root_identity, uid) {
            return Vec::new();
        }
        found.insert(pid, child_identity);
        pending.extend(children);
    }

    after_walk();
    if !identity_is_current(root, root_identity, uid) {
        return Vec::new();
    }

    found
        .into_values()
        .filter(|identity| identity_is_current(root, *identity, uid))
        .collect()
}

fn identity_is_current(root: &Path, identity: ProcessIdentity, uid: u32) -> bool {
    process_identity(root, identity.pid) == Some(identity)
        && process_uid(root, identity.pid) == Some(uid)
}

pub fn process_uid(root: &Path, pid: i32) -> Option<u32> {
    if pid <= 0 {
        return None;
    }
    #[cfg(target_os = "macos")]
    if is_live_proc_root(root) {
        if let Some(info) = darwin_bsd_info(pid) {
            return Some(info.pbi_ruid);
        }
        if let Some(short) = darwin_bsd_short_info(pid) {
            return Some(short.pbsi_ruid);
        }
        return None;
    }
    process_uid_from_proc_fs(root, pid)
}

fn process_uid_from_proc_fs(root: &Path, pid: i32) -> Option<u32> {
    process_ruid_euid_from_proc_fs(root, pid).map(|(ruid, _)| ruid)
}

fn process_ruid_euid_from_proc_fs(root: &Path, pid: i32) -> Option<(u32, u32)> {
    let status = std::fs::read_to_string(root.join(pid.to_string()).join("status")).ok()?;
    status.lines().find_map(|line| {
        let mut values = line.strip_prefix("Uid:")?.split_whitespace();
        let ruid = values.next()?.parse().ok()?;
        let euid = values.next()?.parse().ok()?;
        Some((ruid, euid))
    })
}

fn proc_fs_owned_identity(root: &Path, pid: i32) -> Option<ProcessIdentity> {
    #[cfg(unix)]
    {
        let my_ruid = rustix::process::getuid().as_raw();
        let my_euid = rustix::process::geteuid().as_raw();
        let (ruid, euid) = process_ruid_euid_from_proc_fs(root, pid)?;
        if ruid != my_ruid || euid != my_euid {
            return None;
        }
        process_identity_from_proc_fs(root, pid)
    }
    #[cfg(not(unix))]
    {
        let _ = (root, pid);
        None
    }
}

pub fn read_marker_environment(root: &Path, pid: i32, key: &str) -> Option<Zeroizing<Vec<u8>>> {
    if pid <= 0 {
        return None;
    }
    #[cfg(target_os = "macos")]
    if is_live_proc_root(root) {
        return darwin_read_marker_environment(pid, key);
    }
    let path = root.join(pid.to_string()).join("environ");
    let file = std::fs::File::open(path).ok()?;
    let mut bytes = Zeroizing::new(Vec::with_capacity(4096));
    file.take((MAX_ENVIRONMENT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .ok()?;
    parse_marker_environment(&bytes, key)
}

pub fn read_selected_environment(
    root: &Path,
    pid: i32,
    names: &[&str],
) -> BTreeMap<String, String> {
    if pid <= 0 || names.is_empty() {
        return BTreeMap::new();
    }
    #[cfg(target_os = "macos")]
    if is_live_proc_root(root) {
        return darwin_read_selected_environment(pid, names);
    }
    let Some(before) = proc_fs_owned_identity(root, pid) else {
        return BTreeMap::new();
    };
    let path = root.join(pid.to_string()).join("environ");
    let Ok(file) = std::fs::File::open(path) else {
        return BTreeMap::new();
    };
    let mut bytes = Zeroizing::new(Vec::with_capacity(4096));
    if file
        .take((MAX_ENVIRONMENT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .is_err()
    {
        return BTreeMap::new();
    }
    let selected = parse_nul_environment(&bytes, names).unwrap_or_default();
    if proc_fs_owned_identity(root, pid) != Some(before) {
        return BTreeMap::new();
    }
    selected
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

/// Parses the `argv` slice from a Darwin `KERN_PROCARGS2` buffer.
pub fn parse_procargs2_arguments(bytes: &[u8]) -> Option<Vec<String>> {
    parse_procargs2_layout(bytes).map(|(arguments, _)| arguments)
}

/// Parses selected environment keys from a Darwin `KERN_PROCARGS2` buffer,
/// stopping before trailing Apple vector strings.
pub fn parse_procargs2_environment(
    bytes: &[u8],
    names: &[&str],
) -> Option<BTreeMap<String, String>> {
    let suffix = procargs2_environment_slice(bytes)?;
    parse_nul_environment(suffix, names)
}

/// Extracts the ownership marker value from a Darwin `KERN_PROCARGS2` buffer.
pub fn parse_procargs2_marker(bytes: &[u8], key: &str) -> Option<Zeroizing<Vec<u8>>> {
    let suffix = procargs2_environment_slice(bytes)?;
    parse_marker_environment(suffix, key)
}

fn parse_procargs2_layout(bytes: &[u8]) -> Option<(Vec<String>, usize)> {
    if bytes.len() < 4 || bytes.len() > MAX_ENVIRONMENT_BYTES {
        return None;
    }
    let argc = i32::from_le_bytes(bytes[..4].try_into().ok()?);
    if argc < 0 || (argc as usize) > bytes.len() {
        return None;
    }
    let argc = argc as usize;
    let mut offset = 4;
    let exec_rel_end = bytes[offset..].iter().position(|&b| b == 0)?;
    offset += exec_rel_end + 1;
    while offset < bytes.len() && bytes[offset] == 0 {
        offset += 1;
    }

    let mut arguments = Vec::with_capacity(argc);
    for _ in 0..argc {
        if offset >= bytes.len() {
            return None;
        }
        let rel_end = bytes[offset..].iter().position(|&b| b == 0)?;
        let end = offset + rel_end;
        let argument = std::str::from_utf8(&bytes[offset..end]).ok()?.to_owned();
        arguments.push(argument);
        offset = end + 1;
    }
    Some((arguments, offset))
}

fn procargs2_environment_slice(bytes: &[u8]) -> Option<&[u8]> {
    let (_, env_offset) = parse_procargs2_layout(bytes)?;
    let suffix = bytes.get(env_offset..)?;
    if suffix.first().copied().unwrap_or(0) == 0 {
        return None;
    }
    let mut offset = 0;
    while offset < suffix.len() {
        let rel_term = suffix[offset..].iter().position(|&b| b == 0)?;
        let term = offset + rel_term;
        if term == offset {
            return Some(&suffix[..offset]);
        }
        offset = term + 1;
    }
    Some(suffix)
}

fn parse_nul_environment(bytes: &[u8], names: &[&str]) -> Option<BTreeMap<String, String>> {
    if bytes.len() > MAX_ENVIRONMENT_BYTES || (!bytes.is_empty() && bytes.last() != Some(&0)) {
        return None;
    }
    let mut selected = BTreeMap::new();
    let mut remainder = bytes;
    while let Some(first_non_nul) = remainder.iter().position(|&b| b != 0) {
        remainder = &remainder[first_non_nul..];
        let end = remainder.iter().position(|&b| b == 0)?;
        let record = &remainder[..end];
        remainder = &remainder[end + 1..];
        let separator = record.iter().position(|&b| b == b'=')?;
        let name = std::str::from_utf8(&record[..separator]).ok()?;
        if !names.contains(&name) {
            continue;
        }
        let value = std::str::from_utf8(&record[separator + 1..]).ok()?;
        if let Some(previous) = selected.get(name)
            && previous != value
        {
            return None;
        }
        selected.insert(name.to_owned(), value.to_owned());
    }
    Some(selected)
}

#[cfg(target_os = "macos")]
fn darwin_bsd_info(pid: i32) -> Option<proc_pidinfo::ProcBSDInfo> {
    let pid = u32::try_from(pid).ok()?;
    if pid == 0 {
        return None;
    }
    proc_pidinfo::proc_pidinfo::<proc_pidinfo::ProcBSDInfo>(proc_pidinfo::Pid(pid))
        .ok()
        .flatten()
}

#[cfg(target_os = "macos")]
fn darwin_bsd_short_info(pid: i32) -> Option<proc_pidinfo::ProcBSDShortInfo> {
    let pid = u32::try_from(pid).ok()?;
    if pid == 0 {
        return None;
    }
    proc_pidinfo::proc_pidinfo::<proc_pidinfo::ProcBSDShortInfo>(proc_pidinfo::Pid(pid))
        .ok()
        .flatten()
}

#[cfg(target_os = "macos")]
fn darwin_is_zombie_child(pid: i32) -> bool {
    use rustix::process::{Pid, WaitId, WaitIdOptions, test_kill_process, waitid};

    let Some(raw_pid) = Pid::from_raw(pid) else {
        return false;
    };
    if matches!(
        waitid(
            WaitId::Pid(raw_pid),
            WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
        ),
        Ok(Some(_))
    ) {
        return true;
    }
    test_kill_process(raw_pid).is_ok()
        && darwin_bsd_info(pid).is_none()
        && darwin_bsd_short_info(pid).is_none()
}

#[cfg(target_os = "macos")]
fn darwin_identity_from_bsd(pid: i32, info: &proc_pidinfo::ProcBSDInfo) -> ProcessIdentity {
    let start_ticks = info
        .pbi_start_tvsec
        .saturating_mul(1_000_000)
        .saturating_add(info.pbi_start_tvusec);
    ProcessIdentity { pid, start_ticks }
}

#[cfg(target_os = "macos")]
fn darwin_status_char(status: u32) -> char {
    match status {
        1 => 'I',
        2 => 'R',
        3 => 'S',
        4 => 'T',
        5 => 'Z',
        _ => 'S',
    }
}

#[cfg(target_os = "macos")]
static DARWIN_SYSTEM: std::sync::LazyLock<std::sync::Mutex<sysinfo::System>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(sysinfo::System::new()));

#[cfg(target_os = "macos")]
static DARWIN_ENVIRON_SYSTEM: std::sync::LazyLock<std::sync::Mutex<sysinfo::System>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(sysinfo::System::new()));

#[cfg(target_os = "macos")]
const DARWIN_NO_MARKER_CACHE_MAX: usize = 1024;
#[cfg(target_os = "macos")]
const DARWIN_NO_MARKER_MIN_AGE_MICROS: u64 = 2_000_000;

#[cfg(target_os = "macos")]
static DARWIN_ARGV_SYSTEM: std::sync::LazyLock<std::sync::Mutex<sysinfo::System>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(sysinfo::System::new()));

#[cfg(target_os = "macos")]
static DARWIN_NO_MARKER_CACHE: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashSet<(ProcessIdentity, String)>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashSet::new()));

/// Resolves a Darwin process's `argv` from `KERN_PROCARGS2` arguments or falls back to
/// the executable path (`proc_pidpath`) when `argv` is unavailable.
pub fn resolve_darwin_argv(cmd: Option<Vec<String>>, exe: Option<&Path>) -> Option<Vec<String>> {
    if let Some(args) = cmd
        && !args.is_empty()
    {
        return Some(args);
    }
    exe.filter(|path| !path.as_os_str().is_empty())
        .map(|path| vec![path.to_string_lossy().into_owned()])
}

#[cfg(target_os = "macos")]
pub fn darwin_process_argv(pid: i32) -> Option<Vec<String>> {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, UpdateKind};

    let pid_u32 = u32::try_from(pid).ok()?;
    if pid_u32 == 0 {
        return None;
    }
    let before_info = darwin_bsd_info(pid)?;
    let before_identity = darwin_identity_from_bsd(pid, &before_info);
    let sys_pid = Pid::from_u32(pid_u32);
    let argv = {
        let mut system = DARWIN_ARGV_SYSTEM
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[sys_pid]),
            true,
            ProcessRefreshKind::nothing()
                .with_cmd(UpdateKind::Always)
                .with_exe(UpdateKind::Always),
        );
        let resolved = system.process(sys_pid).and_then(|process| {
            let cmd = (!process.cmd().is_empty()).then(|| {
                process
                    .cmd()
                    .iter()
                    .map(|arg| arg.to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
            });
            resolve_darwin_argv(cmd, process.exe())
        });
        *system = sysinfo::System::new();
        resolved
    }?;
    let after_info = darwin_bsd_info(pid)?;
    if darwin_identity_from_bsd(pid, &after_info) != before_identity {
        return None;
    }
    Some(argv)
}

#[cfg(target_os = "macos")]
fn darwin_same_uid_pids(uid: u32) -> Vec<i32> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate};

    let mut system = DARWIN_SYSTEM
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    let mut pids = system
        .processes()
        .iter()
        .filter_map(|(pid, process)| {
            let pid = i32::try_from(pid.as_u32()).ok().filter(|&pid| pid > 0)?;
            let matches_uid = process.user_id().is_none_or(|id| **id == uid);
            matches_uid.then_some(pid)
        })
        .collect::<Vec<_>>();
    pids.sort_unstable();
    pids.dedup();
    pids
}

#[cfg(target_os = "macos")]
fn darwin_process_children(pid: i32) -> Vec<i32> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate};

    let candidates = {
        let mut system = DARWIN_SYSTEM
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing(),
        );
        system
            .processes()
            .iter()
            .filter_map(|(candidate_pid, process)| {
                let candidate = i32::try_from(candidate_pid.as_u32())
                    .ok()
                    .filter(|&candidate| candidate > 0 && candidate != pid)?;
                let maybe_child = process
                    .parent()
                    .is_none_or(|parent| i32::try_from(parent.as_u32()).ok() == Some(pid));
                maybe_child.then_some(candidate)
            })
            .collect::<Vec<_>>()
    };
    let mut children = BTreeSet::new();
    for candidate in candidates {
        let is_child = darwin_bsd_info(candidate)
            .map(|info| info.pbi_ppid.0 as i32 == pid)
            .or_else(|| darwin_bsd_short_info(candidate).map(|info| info.pbsi_ppid.0 as i32 == pid))
            .unwrap_or(false);
        if is_child {
            children.insert(candidate);
        }
    }
    children.into_iter().collect()
}

#[cfg(target_os = "macos")]
fn darwin_fd_targets(pid: i32) -> Vec<PathBuf> {
    use proc_pidinfo::{
        Pid, PipeFdInfo, ProcFDInfo, ProcFDType, proc_pidfdinfo, proc_pidinfo_list,
    };

    let Ok(pid_u32) = u32::try_from(pid) else {
        return Vec::new();
    };
    let target_pid = Pid(pid_u32);
    let Ok(fds) = proc_pidinfo_list::<ProcFDInfo>(target_pid) else {
        return Vec::new();
    };
    let mut targets = Vec::new();
    for fd in fds {
        if fd.fd_type() == Ok(ProcFDType::PIPE)
            && let Ok(Some(pipe)) = proc_pidfdinfo::<PipeFdInfo>(target_pid, fd.proc_fd)
        {
            for handle in [pipe.pipe_info.pipe_handle, pipe.pipe_info.pipe_peerhandle] {
                if handle != 0 {
                    targets.push(PathBuf::from(format!("pipe:[{handle:x}]")));
                }
            }
        }
    }
    targets.sort();
    targets.dedup();
    targets
}

#[cfg(target_os = "macos")]
fn darwin_owned_identity(pid: i32) -> Option<ProcessIdentity> {
    let my_ruid = rustix::process::getuid().as_raw();
    let my_euid = rustix::process::geteuid().as_raw();
    let info = darwin_bsd_info(pid)?;
    if info.pbi_ruid != my_ruid || info.pbi_uid != my_euid {
        return None;
    }
    Some(darwin_identity_from_bsd(pid, &info))
}

#[cfg(target_os = "macos")]
fn darwin_process_cwd(pid: i32) -> Option<PathBuf> {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, UpdateKind};

    let pid_u32 = u32::try_from(pid).ok()?;
    if pid_u32 == 0 {
        return None;
    }
    let before_identity = darwin_owned_identity(pid)?;
    let sys_pid = Pid::from_u32(pid_u32);
    let cwd = {
        let mut system = DARWIN_ARGV_SYSTEM
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[sys_pid]),
            true,
            ProcessRefreshKind::nothing().with_cwd(UpdateKind::Always),
        );
        let resolved = system
            .process(sys_pid)
            .and_then(|process| process.cwd().map(Path::to_path_buf));
        *system = sysinfo::System::new();
        resolved
    }?;
    if darwin_owned_identity(pid) != Some(before_identity) {
        return None;
    }
    (!cwd.as_os_str().is_empty()).then_some(cwd)
}

#[cfg(target_os = "macos")]
fn darwin_read_selected_environment(pid: i32, names: &[&str]) -> BTreeMap<String, String> {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, UpdateKind};

    let Ok(pid_u32) = u32::try_from(pid) else {
        return BTreeMap::new();
    };
    if pid_u32 == 0 {
        return BTreeMap::new();
    }
    let Some(before_identity) = darwin_owned_identity(pid) else {
        return BTreeMap::new();
    };
    let sys_pid = Pid::from_u32(pid_u32);
    let selected = {
        let mut system = DARWIN_ENVIRON_SYSTEM
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[sys_pid]),
            true,
            ProcessRefreshKind::nothing().with_environ(UpdateKind::Always),
        );
        let result = system
            .process(sys_pid)
            .map(|process| {
                let mut map = BTreeMap::new();
                for entry in process.environ() {
                    let Some(record) = entry.to_str() else {
                        continue;
                    };
                    let Some((name, value)) = record.split_once('=') else {
                        continue;
                    };
                    if names.contains(&name) {
                        map.insert(name.to_owned(), value.to_owned());
                    }
                }
                map
            })
            .unwrap_or_default();
        *system = sysinfo::System::new();
        result
    };
    if darwin_owned_identity(pid) != Some(before_identity) {
        return BTreeMap::new();
    }
    selected
}

#[cfg(target_os = "macos")]
fn darwin_read_marker_environment(pid: i32, key: &str) -> Option<Zeroizing<Vec<u8>>> {
    use std::os::unix::ffi::OsStrExt;
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, UpdateKind};

    let pid_u32 = u32::try_from(pid).ok()?;
    if pid_u32 == 0 {
        return None;
    }
    let info = darwin_bsd_info(pid)?;
    let identity = darwin_identity_from_bsd(pid, &info);
    {
        let cache = DARWIN_NO_MARKER_CACHE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if cache.contains(&(identity, key.to_owned())) {
            return None;
        }
    }
    let sys_pid = Pid::from_u32(pid_u32);
    let marker = {
        let mut system = DARWIN_ENVIRON_SYSTEM
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[sys_pid]),
            true,
            ProcessRefreshKind::nothing().with_environ(UpdateKind::Always),
        );
        let result = system.process(sys_pid).and_then(|process| {
            let prefix = format!("{key}=");
            let prefix_bytes = prefix.as_bytes();
            let mut marker: Option<Zeroizing<Vec<u8>>> = None;
            for entry in process.environ() {
                let bytes = entry.as_bytes();
                let Some(value) = bytes.strip_prefix(prefix_bytes) else {
                    continue;
                };
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
        });
        *system = sysinfo::System::new();
        result
    };
    if marker.is_none() {
        let now_micros = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .and_then(|d| u64::try_from(d.as_micros()).ok())
            .unwrap_or(0);
        if now_micros.saturating_sub(identity.start_ticks) >= DARWIN_NO_MARKER_MIN_AGE_MICROS {
            let mut cache = DARWIN_NO_MARKER_CACHE
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if cache.len() >= DARWIN_NO_MARKER_CACHE_MAX {
                cache.clear();
            }
            cache.insert((identity, key.to_owned()));
        }
    }
    marker
}

#[cfg(all(test, unix))]
mod tests {
    use super::{
        ProcessIdentity, process_cwd, process_descendants_with_hook, read_selected_environment,
    };

    #[test]
    fn descendant_walk_rejects_a_stale_root_identity() {
        let proc_root = tempfile::tempdir().unwrap();
        let uid = rustix::process::getuid().as_raw();
        write_fake_process(proc_root.path(), 500, 5_000, &[501], uid);
        write_fake_process(proc_root.path(), 501, 5_001, &[], uid);

        let descendants = process_descendants_with_hook(
            proc_root.path(),
            ProcessIdentity {
                pid: 500,
                start_ticks: 4_999,
            },
            uid,
            || {},
        );

        assert!(descendants.is_empty());
    }

    #[test]
    fn descendant_walk_discards_results_if_root_identity_changes_during_scan() {
        let proc_root = tempfile::tempdir().unwrap();
        let uid = rustix::process::getuid().as_raw();
        write_fake_process(proc_root.path(), 500, 5_000, &[501], uid);
        write_fake_process(proc_root.path(), 501, 5_001, &[], uid);

        let descendants = process_descendants_with_hook(
            proc_root.path(),
            ProcessIdentity {
                pid: 500,
                start_ticks: 5_000,
            },
            uid,
            || write_fake_process(proc_root.path(), 500, 6_000, &[], uid),
        );

        assert!(descendants.is_empty());
    }

    // Rust-added: verify `process_cwd` and `read_selected_environment` enforce ruid/euid
    // ownership equality and identity presence on `/proc`.
    #[test]
    fn process_cwd_and_selected_env_enforce_ruid_euid_and_identity() {
        let proc_root = tempfile::tempdir().unwrap();
        let ruid = rustix::process::getuid().as_raw();
        let euid = rustix::process::geteuid().as_raw();
        let other_uid = ruid.wrapping_add(1);

        write_fake_process_with_uids(proc_root.path(), 701, 9_000, &[], ruid, euid);
        let p701 = proc_root.path().join("701");
        std::os::unix::fs::symlink("/workspace/alpha", p701.join("cwd")).unwrap();
        std::fs::write(
            p701.join("environ"),
            b"PI_CODING_AGENT_DIR=/custom/pi\0UNRELATED=1\0",
        )
        .unwrap();

        assert_eq!(
            process_cwd(proc_root.path(), 701).as_deref(),
            Some(std::path::Path::new("/workspace/alpha"))
        );
        assert_eq!(
            read_selected_environment(proc_root.path(), 701, &["PI_CODING_AGENT_DIR"])
                .get("PI_CODING_AGENT_DIR")
                .map(String::as_str),
            Some("/custom/pi")
        );

        // Mismatched effective UID must be rejected.
        write_fake_process_with_uids(proc_root.path(), 701, 9_000, &[], ruid, other_uid);
        assert_eq!(process_cwd(proc_root.path(), 701), None);
        assert!(
            read_selected_environment(proc_root.path(), 701, &["PI_CODING_AGENT_DIR"]).is_empty()
        );

        // Mismatched real UID must be rejected.
        write_fake_process_with_uids(proc_root.path(), 701, 9_000, &[], other_uid, euid);
        assert_eq!(process_cwd(proc_root.path(), 701), None);
        assert!(
            read_selected_environment(proc_root.path(), 701, &["PI_CODING_AGENT_DIR"]).is_empty()
        );
    }

    fn write_fake_process(
        root: &std::path::Path,
        pid: i32,
        start_ticks: u64,
        children: &[i32],
        uid: u32,
    ) {
        write_fake_process_with_uids(root, pid, start_ticks, children, uid, uid);
    }

    fn write_fake_process_with_uids(
        root: &std::path::Path,
        pid: i32,
        start_ticks: u64,
        children: &[i32],
        ruid: u32,
        euid: u32,
    ) {
        let process = root.join(pid.to_string());
        let task = process.join("task").join(pid.to_string());
        std::fs::create_dir_all(&task).unwrap();
        std::fs::write(
            task.join("children"),
            children
                .iter()
                .map(i32::to_string)
                .collect::<Vec<_>>()
                .join(" "),
        )
        .unwrap();
        let mut fields = vec!["0".to_owned(); 50];
        fields[0] = "S".to_owned();
        fields[1] = "1".to_owned();
        fields[2] = pid.to_string();
        fields[19] = start_ticks.to_string();
        std::fs::write(
            process.join("stat"),
            format!("{pid} (synthetic process) {}\n", fields.join(" ")),
        )
        .unwrap();
        std::fs::write(
            process.join("status"),
            format!("Name:\tsynthetic\nUid:\t{ruid}\t{euid}\t{euid}\t{euid}\n"),
        )
        .unwrap();
    }
}
