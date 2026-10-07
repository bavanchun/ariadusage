// Ported from CodexBar Sources/CodexBarCore/Providers/Antigravity/AntigravityStatusProbe+PortDetection.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::{BTreeSet, HashSet};
use std::path::Path;

/// Extracts the socket inode number from a `/proc/<pid>/fd` symlink destination
/// such as `socket:[12345]`. Returns `None` for non-socket descriptors.
pub fn socket_inode(link: &str) -> Option<u64> {
    let stripped = link.strip_prefix("socket:[")?.strip_suffix(']')?;
    if stripped.is_empty() {
        return None;
    }
    stripped.parse::<u64>().ok()
}

/// Returns the local ports of LISTEN sockets whose inode is in `socket_inodes`.
///
/// `content` is the raw text of a process-scoped `tcp` or `tcp6` table. Each
/// row encodes the local endpoint as `ADDRESS:PORT` and the owning socket inode
/// in column ten (index 9).
pub fn parse_tcp_table(content: &str, socket_inodes: &HashSet<u64>) -> BTreeSet<u16> {
    let mut ports = BTreeSet::new();
    for line in content.lines() {
        let columns: Vec<&str> = line.split_whitespace().collect();
        // Columns: sl local_address rem_address st tx_queue:rx_queue tr:tm->when retrnsmt uid timeout inode
        if columns.len() <= 9 {
            continue;
        }
        if !columns[3].eq_ignore_ascii_case("0A") {
            continue;
        }
        let Ok(inode) = columns[9].parse::<u64>() else {
            continue;
        };
        if !socket_inodes.contains(&inode) {
            continue;
        }
        let local_address = columns[1];
        let Some((_, hex_port)) = local_address.rsplit_once(':') else {
            continue;
        };
        let Ok(port_u32) = u32::from_str_radix(hex_port, 16) else {
            continue;
        };
        if let Ok(port) = u16::try_from(port_u32) {
            ports.insert(port);
        }
    }
    ports
}

/// Collects the socket inodes referenced by the process's open file descriptors.
pub fn process_socket_inodes(proc_root: &Path, pid: i32) -> HashSet<u64> {
    let targets = crate::brokers::process::fd_targets(proc_root, pid);
    let mut inodes = HashSet::with_capacity(targets.len());
    for target in targets {
        if let Some(inode) = socket_inode(&target.to_string_lossy()) {
            inodes.insert(inode);
        }
    }
    inodes
}

/// Recovers the listening ports owned by `pid` by matching its open socket
/// inodes against the TCP tables from the same process/network namespace.
pub fn proc_listening_ports(proc_root: &Path, pid: i32) -> Vec<u16> {
    if pid <= 0 {
        return Vec::new();
    }
    let inodes = process_socket_inodes(proc_root, pid);
    if inodes.is_empty() {
        return Vec::new();
    }
    let process_root = proc_root.join(pid.to_string());
    let mut ports = BTreeSet::new();
    for table_name in ["net/tcp", "net/tcp6"] {
        let path = process_root.join(table_name);
        if let Ok(content) = std::fs::read_to_string(&path) {
            ports.extend(parse_tcp_table(&content, &inodes));
        }
    }
    ports.into_iter().collect()
}
