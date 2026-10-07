// Ported from CodexBar TestsLinux/ProcNetTCPListeningPortParserLinuxTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
#![cfg(target_os = "linux")]

use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::symlink;

use ariadusage_engine::brokers::local_probe::proc_net::{
    parse_tcp_table, proc_listening_ports, socket_inode,
};
use tempfile::tempdir;

const SAMPLE: &str = "\
  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n\
   0: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 111111 1 0000000000000000 100 0 0 10 0\n\
   1: 0100007F:C000 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 222222 1 0000000000000000 100 0 0 10 0\n\
   2: 0100007F:1F91 0100007F:E1F0 01 00000000:00000000 00:00000000 00000000  1000        0 333333 1 0000000000000000 100 0 0 10 0\n";

#[test]
// CodexBar: TestsLinux/ProcNetTCPListeningPortParserLinuxTests.swift:18
fn returns_listening_ports_for_owned_socket_inodes() {
    let inodes = HashSet::from([111111, 222222]);
    let ports = parse_tcp_table(SAMPLE, &inodes);
    let port_list: Vec<u16> = ports.into_iter().collect();
    assert_eq!(port_list, vec![8080, 49152]);
}

#[test]
// CodexBar: TestsLinux/ProcNetTCPListeningPortParserLinuxTests.swift:25
fn parses_tcp6_and_deduplicates_ports_across_tables() {
    let tcp6 = "\
  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n\
   0: 00000000000000000000000000000000:C000 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 222222\n";
    let inodes = HashSet::from([222222]);
    let tcp_ports = parse_tcp_table(SAMPLE, &inodes);
    let tcp6_ports = parse_tcp_table(tcp6, &inodes);
    let union: Vec<u16> = tcp_ports.union(&tcp6_ports).copied().collect();
    assert_eq!(union, vec![49152]);
}

#[test]
// CodexBar: TestsLinux/ProcNetTCPListeningPortParserLinuxTests.swift:38
fn ignores_malformed_and_out_of_range_ports() {
    let malformed = "\
  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n\
   0: 0100007F:NOTHEX 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 111111\n\
   1: 0100007F:10000 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 111111\n\
   2: missing-columns\n";
    let inodes = HashSet::from([111111]);
    assert!(parse_tcp_table(malformed, &inodes).is_empty());
    assert!(parse_tcp_table("header only", &inodes).is_empty());
}

#[test]
// CodexBar: TestsLinux/ProcNetTCPListeningPortParserLinuxTests.swift:52
fn accepts_a_headerless_proc_row() {
    let row = " 0: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 111111 1 0000000000000000 100 0 0 10 0\n";
    let inodes = HashSet::from([111111]);
    let ports = parse_tcp_table(row, &inodes);
    let port_list: Vec<u16> = ports.into_iter().collect();
    assert_eq!(port_list, vec![8080]);
}

#[test]
// CodexBar: TestsLinux/ProcNetTCPListeningPortParserLinuxTests.swift:59
fn ignores_listening_sockets_owned_by_other_processes() {
    let inodes = HashSet::from([999999]);
    let ports = parse_tcp_table(SAMPLE, &inodes);
    assert!(ports.is_empty());
}

#[test]
// CodexBar: TestsLinux/ProcNetTCPListeningPortParserLinuxTests.swift:66
fn ignores_non_listening_sockets() {
    let inodes = HashSet::from([333333]);
    let ports = parse_tcp_table(SAMPLE, &inodes);
    assert!(ports.is_empty());
}

#[test]
// CodexBar: TestsLinux/ProcNetTCPListeningPortParserLinuxTests.swift:74
fn parses_socket_inode_from_fd_symlink_destination() {
    assert_eq!(socket_inode("socket:[12345]"), Some(12345));
    assert_eq!(socket_inode("/dev/pts/0"), None);
    assert_eq!(socket_inode("anon_inode:[eventpoll]"), None);
    assert_eq!(socket_inode("socket:[]"), None);
}

#[test]
// CodexBar: TestsLinux/ProcNetTCPListeningPortParserLinuxTests.swift:82
fn reads_process_scoped_tcp_tables() {
    let proc_root = tempdir().unwrap();
    let process_root = proc_root.path().join("42");
    let fd_dir = process_root.join("fd");
    let net_dir = process_root.join("net");
    let caller_net_dir = proc_root.path().join("net");

    fs::create_dir_all(&fd_dir).unwrap();
    fs::create_dir_all(&net_dir).unwrap();
    fs::create_dir_all(&caller_net_dir).unwrap();

    symlink("socket:[111111]", fd_dir.join("7")).unwrap();
    fs::write(net_dir.join("tcp"), SAMPLE).unwrap();
    fs::write(caller_net_dir.join("tcp"), SAMPLE.replace(":1F90", ":C001")).unwrap();

    let ports = proc_listening_ports(proc_root.path(), 42);
    assert_eq!(ports, vec![8080]);
}
