// Ported from CodexBar Tests/CodexBarTests/ExecutableSearchSecurityTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use ariadusage_engine::brokers::exec_resolver::{
    FileOwnerAndMode, ResolveError, ResolverFs, ResolverInputs, Tool, effective_path, resolve,
    resolve_codex_for_rpc,
};
use ariadusage_engine::brokers::process::ProcessEnv;

struct MockSecurityFs {
    executables: HashSet<PathBuf>,
    owners_and_modes: HashMap<PathBuf, FileOwnerAndMode>,
    current_uid: u32,
}

impl MockSecurityFs {
    fn new() -> Self {
        Self {
            executables: HashSet::new(),
            owners_and_modes: HashMap::new(),
            current_uid: 1000,
        }
    }

    fn add_executable(&mut self, path: impl AsRef<Path>, uid: u32, mode: u32) {
        let p = path.as_ref().to_path_buf();
        self.executables.insert(p.clone());
        self.owners_and_modes
            .insert(p, FileOwnerAndMode { uid, mode });
    }

    fn set_dir_metadata(&mut self, path: impl AsRef<Path>, uid: u32, mode: u32) {
        let p = path.as_ref().to_path_buf();
        self.owners_and_modes
            .insert(p, FileOwnerAndMode { uid, mode });
    }
}

impl ResolverFs for MockSecurityFs {
    fn is_executable(&self, path: &Path) -> bool {
        self.executables.contains(path)
    }

    fn owner_and_mode(&self, path: &Path) -> Option<FileOwnerAndMode> {
        if let Some(om) = self.owners_and_modes.get(path) {
            return Some(*om);
        }
        // If path is a parent directory of a registered path, default to trusted (1000, 0755)
        Some(FileOwnerAndMode {
            uid: self.current_uid,
            mode: 0o755,
        })
    }

    fn current_uid(&self) -> u32 {
        self.current_uid
    }
}

#[test]
// CodexBar: ExecutableSearchSecurityTests.swift:25
fn implicit_lookup_ignores_relative_path_entries() {
    let mut fs = MockSecurityFs::new();
    let relative_path = PathBuf::from("tools/claude");
    fs.add_executable(&relative_path, 1000, 0o755);

    let home = Path::new("/fakehome");
    let env = ProcessEnv::from_iter([("PATH", ".:tools:../bin")]);
    let login_path = [PathBuf::from("."), PathBuf::from("tools")];

    let inputs = ResolverInputs::new(&env, home, &fs).with_login_path(&login_path);
    let result = resolve(Tool::Claude, &inputs);
    assert_eq!(result, Err(ResolveError::NotFound));
}

#[test]
// CodexBar: ExecutableSearchSecurityTests.swift:47
fn explicit_relative_shell_and_pty_executables_remain_usable() {
    let mut fs = MockSecurityFs::new();
    let explicit_relative = "relative/bin/synthetic-tool";
    let cur_dir = std::env::current_dir().unwrap();
    let absolute_path = cur_dir.join(explicit_relative);
    fs.add_executable(&absolute_path, 1000, 0o755);

    let home = Path::new("/fakehome");
    let env = ProcessEnv::empty();
    let inputs = ResolverInputs::new(&env, home, &fs);

    let resolution = resolve_codex_for_rpc(&inputs, explicit_relative, || None)
        .expect("explicit relative made absolute should succeed");
    assert_eq!(resolution.executable.as_path(), absolute_path.as_path());
}

#[test]
// CodexBar: ExecutableSearchSecurityTests.swift:74
fn child_path_removes_working_directory_entries_and_retains_absolute_install_paths() {
    let login_path = [
        PathBuf::from(""),
        PathBuf::from("."),
        PathBuf::from("bin"),
        PathBuf::from("/custom/bin"),
    ];
    let env_path = OsString::from(":.:tools:../bin:/usr/bin:");

    let path = effective_path(Some(&login_path), Some(&env_path));
    assert_eq!(path.to_string_lossy(), "/custom/bin:/usr/bin");

    let empty_login: Option<&[PathBuf]> = None;
    let relative_env = OsString::from(".:bin:");
    let fallback = effective_path(empty_login, Some(&relative_env));
    assert_eq!(fallback.to_string_lossy(), "/usr/bin:/bin:/usr/sbin:/sbin");
}

#[test]
// CodexBar: ExecutableSearchSecurityTests.swift:85
fn child_interpreter_search_ignores_a_planted_relative_path_directory() {
    let relative_planted = "../fake/planted";
    let trusted_path = "/trusted/bin";
    let env_path = OsString::from(format!("{relative_planted}:{trusted_path}"));

    let cleaned = effective_path(None, Some(&env_path));
    assert_eq!(cleaned.to_string_lossy(), trusted_path);
}

#[test]
// CodexBar: ExecutableSearchSecurityTests.swift:105
fn explicit_relative_subprocess_executable_remains_usable() {
    let mut fs = MockSecurityFs::new();
    let relative_exe = "./tools/codex-runner";
    let cur_dir = std::env::current_dir().unwrap();
    let absolute_path = cur_dir.join("tools/codex-runner");
    fs.add_executable(&absolute_path, 1000, 0o755);

    let home = Path::new("/fakehome");
    let env = ProcessEnv::empty();
    let inputs = ResolverInputs::new(&env, home, &fs);

    let resolution = resolve_codex_for_rpc(&inputs, relative_exe, || None).unwrap();
    assert_eq!(resolution.executable.as_path(), absolute_path.as_path());
}

#[test]
fn candidate_in_group_writable_directory_without_sticky_bit_is_untrusted() {
    let mut fs = MockSecurityFs::new();
    let dir = Path::new("/group-writable");
    let candidate = dir.join("claude");
    // Directory is group-writable (0775) without sticky bit (01000)
    fs.set_dir_metadata(dir, 1000, 0o775);
    fs.add_executable(&candidate, 1000, 0o755);

    let home = Path::new("/fakehome");
    let env = ProcessEnv::from_iter([("PATH", "/group-writable")]);
    let inputs = ResolverInputs::new(&env, home, &fs);

    let result = resolve(Tool::Claude, &inputs);
    assert_eq!(result, Err(ResolveError::Untrusted));
}

#[test]
fn candidate_in_world_writable_directory_without_sticky_bit_is_untrusted() {
    let mut fs = MockSecurityFs::new();
    let dir = Path::new("/world-writable");
    let candidate = dir.join("claude");
    // Directory is world-writable (0777) without sticky bit (01000)
    fs.set_dir_metadata(dir, 1000, 0o777);
    fs.add_executable(&candidate, 1000, 0o755);

    let home = Path::new("/fakehome");
    let env = ProcessEnv::from_iter([("PATH", "/world-writable")]);
    let inputs = ResolverInputs::new(&env, home, &fs);

    let result = resolve(Tool::Claude, &inputs);
    assert_eq!(result, Err(ResolveError::Untrusted));
}

#[test]
fn candidate_in_world_writable_directory_with_sticky_bit_is_trusted_if_file_trusted() {
    let mut fs = MockSecurityFs::new();
    let dir = Path::new("/tmp");
    let candidate = dir.join("claude");
    // /tmp is 01777: world-writable WITH sticky bit (01000)
    fs.set_dir_metadata(dir, 0, 0o1777);
    fs.add_executable(&candidate, 1000, 0o755);

    let home = Path::new("/fakehome");
    let env = ProcessEnv::from_iter([("PATH", "/tmp")]);
    let inputs = ResolverInputs::new(&env, home, &fs);

    let result = resolve(Tool::Claude, &inputs).unwrap();
    assert_eq!(result.as_path(), candidate.as_path());
}

#[test]
fn candidate_owned_by_another_uid_is_untrusted() {
    let mut fs = MockSecurityFs::new();
    let dir = Path::new("/custom/bin");
    let candidate = dir.join("claude");
    fs.set_dir_metadata(dir, 1000, 0o755);
    // File is owned by uid 1001 (another non-root user)
    fs.add_executable(&candidate, 1001, 0o755);

    let home = Path::new("/fakehome");
    let env = ProcessEnv::from_iter([("PATH", "/custom/bin")]);
    let inputs = ResolverInputs::new(&env, home, &fs);

    let result = resolve(Tool::Claude, &inputs);
    assert_eq!(result, Err(ResolveError::Untrusted));
}

#[test]
fn candidate_whose_parent_is_owned_by_another_uid_is_untrusted() {
    let mut fs = MockSecurityFs::new();
    let dir = Path::new("/other-user/bin");
    let candidate = dir.join("claude");
    // Directory is owned by uid 1001 (another non-root user)
    fs.set_dir_metadata(dir, 1001, 0o755);
    fs.add_executable(&candidate, 1000, 0o755);

    let home = Path::new("/fakehome");
    let env = ProcessEnv::from_iter([("PATH", "/other-user/bin")]);
    let inputs = ResolverInputs::new(&env, home, &fs);

    let result = resolve(Tool::Claude, &inputs);
    assert_eq!(result, Err(ResolveError::Untrusted));
}

#[test]
fn explicit_override_failing_trust_check_is_error_for_agy_and_falls_through_for_claude_and_codex() {
    let mut fs = MockSecurityFs::new();
    let untrusted_dir = Path::new("/untrusted/bin");
    fs.set_dir_metadata(untrusted_dir, 1001, 0o755); // owned by other uid

    let untrusted_agy = untrusted_dir.join("agy");
    fs.add_executable(&untrusted_agy, 1001, 0o755);

    let untrusted_claude = untrusted_dir.join("claude");
    fs.add_executable(&untrusted_claude, 1001, 0o755);

    let trusted_ambient_claude = Path::new("/ambient/bin/claude");
    fs.set_dir_metadata("/ambient/bin", 1000, 0o755);
    fs.add_executable(trusted_ambient_claude, 1000, 0o755);

    let home = Path::new("/fakehome");

    // 1) Antigravity: untrusted override is an error (Untrusted)
    let env_agy = ProcessEnv::from_iter([
        ("ANTIGRAVITY_CLI_PATH", untrusted_agy.to_str().unwrap()),
        ("PATH", "/ambient/bin"),
    ]);
    let inputs_agy = ResolverInputs::new(&env_agy, home, &fs);
    assert_eq!(
        resolve(Tool::Antigravity, &inputs_agy),
        Err(ResolveError::Untrusted)
    );

    // 2) Claude: untrusted override falls through like non-executable to ambient PATH
    let env_claude = ProcessEnv::from_iter([
        ("CLAUDE_CLI_PATH", untrusted_claude.to_str().unwrap()),
        ("PATH", "/ambient/bin"),
    ]);
    let inputs_claude = ResolverInputs::new(&env_claude, home, &fs);
    let resolved_claude = resolve(Tool::Claude, &inputs_claude).unwrap();
    assert_eq!(resolved_claude.as_path(), trusted_ambient_claude);
}
