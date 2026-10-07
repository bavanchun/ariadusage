// Ported from CodexBar Tests/CodexBarTests/PathBuilderTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/AntigravityBinaryLocatorTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexExecutableResolverTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use ariadusage_engine::brokers::exec_resolver::{
    FileOwnerAndMode, ResolveError, ResolverFs, ResolverInputs, Tool, resolve,
    resolve_codex_for_rpc,
};
use ariadusage_engine::brokers::process::ProcessEnv;

struct MockFs {
    executables: HashSet<PathBuf>,
    scripts: HashSet<PathBuf>,
    owners_and_modes: HashMap<PathBuf, FileOwnerAndMode>,
    current_uid: u32,
}

impl MockFs {
    fn new(executables: &[&str]) -> Self {
        let mut set = HashSet::new();
        for e in executables {
            set.insert(PathBuf::from(e));
        }
        Self {
            executables: set,
            scripts: HashSet::new(),
            owners_and_modes: HashMap::new(),
            current_uid: 1000,
        }
    }

    fn with_script(mut self, path: &str) -> Self {
        let p = PathBuf::from(path);
        self.executables.insert(p.clone());
        self.scripts.insert(p);
        self
    }
}

impl ResolverFs for MockFs {
    fn is_executable(&self, path: &Path) -> bool {
        self.executables.contains(path)
    }

    fn owner_and_mode(&self, path: &Path) -> Option<FileOwnerAndMode> {
        if let Some(om) = self.owners_and_modes.get(path) {
            return Some(*om);
        }
        // Default to trusted: owned by current_uid (1000), mode 0755
        Some(FileOwnerAndMode {
            uid: self.current_uid,
            mode: 0o755,
        })
    }

    fn current_uid(&self) -> u32 {
        self.current_uid
    }

    fn is_script(&self, path: &Path) -> bool {
        self.scripts.contains(path)
    }
}

#[test]
// CodexBar: AntigravityBinaryLocatorTests.swift:7
fn unusable_env_override_fails_without_ambient_fallback() {
    let ambient = "/opt/homebrew/bin/agy";
    let fs = MockFs::new(&[ambient]);
    let home = Path::new("/fakehome");

    for override_val in ["/nonexistent-agy-guard", "", "   ", "agy"] {
        let env = ProcessEnv::from_iter([
            ("ANTIGRAVITY_CLI_PATH", override_val),
            ("PATH", "/opt/homebrew/bin"),
        ]);
        let login_path = [PathBuf::from("/opt/homebrew/bin")];
        let command_v = |_tool: &str| -> Option<PathBuf> {
            panic!("An unusable override must not run shell lookup");
        };
        let alias = |_tool: &str| -> Option<PathBuf> {
            panic!("An unusable override must not run alias lookup");
        };

        let inputs = ResolverInputs::new(&env, home, &fs)
            .with_login_path(&login_path)
            .with_command_v(&command_v)
            .with_alias(&alias);

        let result = resolve(Tool::Antigravity, &inputs);
        assert_eq!(result, Err(ResolveError::OverrideInvalid));
    }
}

#[test]
// CodexBar: AntigravityBinaryLocatorTests.swift:34
fn usable_env_override_is_used_without_other_lookups() {
    let override_path = "/custom/bin/agy";
    let fs = MockFs::new(&[override_path]);
    let home = Path::new("/fakehome");
    let env = ProcessEnv::from_iter([("ANTIGRAVITY_CLI_PATH", override_path)]);

    let command_v = |_tool: &str| -> Option<PathBuf> {
        panic!("A usable override must not run shell lookup");
    };
    let alias = |_tool: &str| -> Option<PathBuf> {
        panic!("A usable override must not run alias lookup");
    };

    let inputs = ResolverInputs::new(&env, home, &fs)
        .with_command_v(&command_v)
        .with_alias(&alias);

    let resolved = resolve(Tool::Antigravity, &inputs).unwrap();
    assert_eq!(resolved.as_path(), Path::new(override_path));
}

#[test]
// CodexBar: AntigravityBinaryLocatorTests.swift:58
fn without_override_well_known_paths_still_resolve() {
    let homebrew_path = "/opt/homebrew/bin/agy";
    let fs = MockFs::new(&[homebrew_path]);
    let home = Path::new("/fakehome");
    let env = ProcessEnv::from_iter([("SHELL", "/bin/zsh")]);

    let command_v = |_tool: &str| -> Option<PathBuf> { None };
    let alias = |_tool: &str| -> Option<PathBuf> { None };

    let inputs = ResolverInputs::new(&env, home, &fs)
        .with_command_v(&command_v)
        .with_alias(&alias);

    let resolved = resolve(Tool::Antigravity, &inputs).unwrap();
    assert_eq!(resolved.as_path(), Path::new(homebrew_path));
}

#[test]
// CodexBar: AntigravityBinaryLocatorTests.swift:76
fn minimal_fallback_preserves_a_hit_or_no_executable() {
    let home = Path::new("/fakehome");
    let env = ProcessEnv::empty();

    // Hit case
    let fs_hit = MockFs::new(&["/bin/agy"]);
    let inputs_hit = ResolverInputs::new(&env, home, &fs_hit);
    let resolved = resolve(Tool::Antigravity, &inputs_hit).unwrap();
    assert_eq!(resolved.as_path(), Path::new("/bin/agy"));

    // Miss case
    let fs_miss = MockFs::new(&[]);
    let inputs_miss = ResolverInputs::new(&env, home, &fs_miss);
    let err = resolve(Tool::Antigravity, &inputs_miss).unwrap_err();
    assert_eq!(err, ResolveError::NotFound);
}

#[test]
// CodexBar: AntigravityBinaryLocatorTests.swift:89
fn other_providers_retain_unusable_override_fallback() {
    let ambient = "/ambient/bin/claude";
    let fs = MockFs::new(&[ambient]);
    let home = Path::new("/fakehome");
    let env = ProcessEnv::from_iter([
        ("CLAUDE_CLI_PATH", "/missing/claude"),
        ("PATH", "/ambient/bin"),
    ]);

    let inputs = ResolverInputs::new(&env, home, &fs);
    let resolved = resolve(Tool::Claude, &inputs).unwrap();
    assert_eq!(resolved.as_path(), Path::new(ambient));
}

#[test]
// CodexBar: PathBuilderTests.swift:126
fn resolves_claude_from_explicit_override() {
    let override_path = "/custom/bin/claude";
    let fs = MockFs::new(&[override_path, "/opt/homebrew/bin/claude"]);
    let home = Path::new("/fakehome");
    let env = ProcessEnv::from_iter([("CLAUDE_CLI_PATH", override_path)]);

    let inputs = ResolverInputs::new(&env, home, &fs);
    let resolved = resolve(Tool::Claude, &inputs).unwrap();
    assert_eq!(resolved.as_path(), Path::new(override_path));
}

#[test]
// CodexBar: PathBuilderTests.swift:139
fn resolves_codex_from_explicit_override() {
    let override_path = "/custom/bin/codex";
    let fs = MockFs::new(&[override_path]);
    let home = Path::new("/fakehome");
    let env = ProcessEnv::from_iter([("CODEX_CLI_PATH", override_path)]);

    let inputs = ResolverInputs::new(&env, home, &fs);
    let resolved = resolve(Tool::Codex, &inputs).unwrap();
    assert_eq!(resolved.as_path(), Path::new(override_path));
}

#[test]
// CodexBar: PathBuilderTests.swift:150
fn resolves_codex_from_well_known_fallback_when_path_has_no_codex() {
    let home = Path::new("/fakehome");
    let shim = "/fakehome/.local/share/mise/shims/codex";
    let fs = MockFs::new(&[shim]);
    let env = ProcessEnv::from_iter([("PATH", "/usr/bin:/bin")]);

    let inputs = ResolverInputs::new(&env, home, &fs);
    let resolved = resolve(Tool::Codex, &inputs).unwrap();
    assert_eq!(resolved.as_path(), Path::new(shim));
}

#[test]
// CodexBar: PathBuilderTests.swift:635
fn resolves_codex_from_interactive_shell() {
    let shell_path = "/shell/bin/codex";
    let fs = MockFs::new(&[shell_path]);
    let home = Path::new("/fakehome");
    let env = ProcessEnv::from_iter([("SHELL", "/bin/zsh")]);

    let command_v = |tool: &str| -> Option<PathBuf> {
        assert_eq!(tool, "codex");
        Some(PathBuf::from(shell_path))
    };

    let inputs = ResolverInputs::new(&env, home, &fs).with_command_v(&command_v);
    let resolved = resolve(Tool::Codex, &inputs).unwrap();
    assert_eq!(resolved.as_path(), Path::new(shell_path));
}

#[test]
// CodexBar: PathBuilderTests.swift:655
fn resolves_claude_from_interactive_shell() {
    let shell_path = "/shell/bin/claude";
    let fs = MockFs::new(&[shell_path]);
    let home = Path::new("/fakehome");
    let env = ProcessEnv::from_iter([("SHELL", "/bin/zsh")]);

    let command_v = |tool: &str| -> Option<PathBuf> {
        assert_eq!(tool, "claude");
        Some(PathBuf::from(shell_path))
    };

    let inputs = ResolverInputs::new(&env, home, &fs).with_command_v(&command_v);
    let resolved = resolve(Tool::Claude, &inputs).unwrap();
    assert_eq!(resolved.as_path(), Path::new(shell_path));
}

#[test]
// CodexBar: PathBuilderTests.swift:695
fn resolves_claude_from_login_path() {
    let login_bin = "/login/bin/claude";
    let env_bin = "/env/bin/claude";
    let fs = MockFs::new(&[login_bin, env_bin]);
    let home = Path::new("/fakehome");
    let env = ProcessEnv::from_iter([("PATH", "/env/bin")]);
    let login_path = [PathBuf::from("/login/bin")];

    let inputs = ResolverInputs::new(&env, home, &fs).with_login_path(&login_path);
    let resolved = resolve(Tool::Claude, &inputs).unwrap();
    assert_eq!(resolved.as_path(), Path::new(login_bin));
}

#[test]
// CodexBar: PathBuilderTests.swift:706
fn resolves_claude_from_alias_when_other_lookups_fail() {
    let alias_path = "/fakehome/.claude/local/bin/claude";
    let fs = MockFs::new(&[alias_path]);
    let home = Path::new("/fakehome");
    let env = ProcessEnv::from_iter([("SHELL", "/bin/zsh")]);
    let alias_called = AtomicBool::new(false);

    let command_v = |_tool: &str| -> Option<PathBuf> { None };
    let alias = |tool: &str| -> Option<PathBuf> {
        alias_called.store(true, Ordering::SeqCst);
        assert_eq!(tool, "claude");
        Some(PathBuf::from(alias_path))
    };

    let inputs = ResolverInputs::new(&env, home, &fs)
        .with_command_v(&command_v)
        .with_alias(&alias);

    let resolved = resolve(Tool::Claude, &inputs).unwrap();
    assert!(alias_called.load(Ordering::SeqCst));
    assert_eq!(resolved.as_path(), Path::new(alias_path));
}

#[test]
// CodexBar: PathBuilderTests.swift:736
fn resolves_codex_from_alias_when_other_lookups_fail() {
    let alias_path = "/fakehome/.codex/bin/codex";
    let fs = MockFs::new(&[alias_path]);
    let home = Path::new("/fakehome");
    let env = ProcessEnv::from_iter([("SHELL", "/bin/zsh")]);
    let alias_called = AtomicBool::new(false);

    let command_v = |_tool: &str| -> Option<PathBuf> { None };
    let alias = |tool: &str| -> Option<PathBuf> {
        alias_called.store(true, Ordering::SeqCst);
        assert_eq!(tool, "codex");
        Some(PathBuf::from(alias_path))
    };

    let inputs = ResolverInputs::new(&env, home, &fs)
        .with_command_v(&command_v)
        .with_alias(&alias);

    let resolved = resolve(Tool::Codex, &inputs).unwrap();
    assert!(alias_called.load(Ordering::SeqCst));
    assert_eq!(resolved.as_path(), Path::new(alias_path));
}

#[test]
// CodexBar: PathBuilderTests.swift:766
fn resolves_claude_from_well_known_paths() {
    let home = Path::new("/fakehome");
    let claude_bin = "/fakehome/.claude/bin/claude";
    let fs = MockFs::new(&[claude_bin]);
    let env = ProcessEnv::from_iter([("SHELL", "/bin/zsh")]);

    let command_v = |_tool: &str| -> Option<PathBuf> { None };
    let alias = |_tool: &str| -> Option<PathBuf> { None };

    let inputs = ResolverInputs::new(&env, home, &fs)
        .with_command_v(&command_v)
        .with_alias(&alias);

    let resolved = resolve(Tool::Claude, &inputs).unwrap();
    assert_eq!(resolved.as_path(), Path::new(claude_bin));
}

#[test]
// CodexBar: PathBuilderTests.swift:800
fn resolves_claude_from_native_installer_path() {
    let home = Path::new("/fakehome");
    let native_path = "/fakehome/.local/bin/claude";
    let fs = MockFs::new(&[native_path]);
    let env = ProcessEnv::from_iter([("SHELL", "/bin/zsh")]);

    let command_v = |_tool: &str| -> Option<PathBuf> { None };
    let alias = |_tool: &str| -> Option<PathBuf> { None };

    let inputs = ResolverInputs::new(&env, home, &fs)
        .with_command_v(&command_v)
        .with_alias(&alias);

    let resolved = resolve(Tool::Claude, &inputs).unwrap();
    assert_eq!(resolved.as_path(), Path::new(native_path));
}

#[test]
// CodexBar: PathBuilderTests.swift:817
fn prefers_migrated_local_claude_path_over_legacy_home_dir_path() {
    let home = Path::new("/fakehome");
    let migrated = "/fakehome/.claude/local/claude";
    let legacy = "/fakehome/.claude/bin/claude";
    let fs = MockFs::new(&[migrated, legacy]);
    let env = ProcessEnv::from_iter([("SHELL", "/bin/zsh")]);

    let inputs = ResolverInputs::new(&env, home, &fs);
    let resolved = resolve(Tool::Claude, &inputs).unwrap();
    assert_eq!(resolved.as_path(), Path::new(migrated));
}

#[test]
// CodexBar: PathBuilderTests.swift:853
fn prefers_homebrew_arm_path_over_usr_local_fallback() {
    let home = Path::new("/fakehome");
    let homebrew = "/opt/homebrew/bin/claude";
    let usr_local = "/usr/local/bin/claude";
    let fs = MockFs::new(&[homebrew, usr_local]);
    let env = ProcessEnv::from_iter([("SHELL", "/bin/zsh")]);

    let inputs = ResolverInputs::new(&env, home, &fs);
    let resolved = resolve(Tool::Claude, &inputs).unwrap();
    assert_eq!(resolved.as_path(), Path::new(homebrew));
}

#[test]
// CodexBar: PathBuilderTests.swift:872
fn prefers_well_known_paths_over_interactive_shell_lookup() {
    let home = Path::new("/fakehome");
    let well_known = "/usr/local/bin/claude";
    let shell_path = "/custom/bin/claude";
    let fs = MockFs::new(&[well_known, shell_path]);
    let env = ProcessEnv::from_iter([("SHELL", "/bin/zsh")]);
    let shell_lookup_called = AtomicBool::new(false);

    let command_v = |_: &str| -> Option<PathBuf> {
        shell_lookup_called.store(true, Ordering::SeqCst);
        Some(PathBuf::from(shell_path))
    };

    let inputs = ResolverInputs::new(&env, home, &fs).with_command_v(&command_v);
    let resolved = resolve(Tool::Claude, &inputs).unwrap();
    assert!(!shell_lookup_called.load(Ordering::SeqCst));
    assert_eq!(resolved.as_path(), Path::new(well_known));
}

#[test]
// CodexBar: PathBuilderTests.swift:893
fn skips_alias_when_command_v_resolves() {
    let home = Path::new("/fakehome");
    let shell_path = "/shell/bin/claude";
    let alias_path = "/fakehome/.claude/local/bin/claude";
    let fs = MockFs::new(&[shell_path, alias_path]);
    let env = ProcessEnv::from_iter([("SHELL", "/bin/zsh")]);
    let alias_called = AtomicBool::new(false);

    let command_v = |_: &str| -> Option<PathBuf> { Some(PathBuf::from(shell_path)) };
    let alias = |_: &str| -> Option<PathBuf> {
        alias_called.store(true, Ordering::SeqCst);
        Some(PathBuf::from(alias_path))
    };

    let inputs = ResolverInputs::new(&env, home, &fs)
        .with_command_v(&command_v)
        .with_alias(&alias);

    let resolved = resolve(Tool::Claude, &inputs).unwrap();
    assert!(!alias_called.load(Ordering::SeqCst));
    assert_eq!(resolved.as_path(), Path::new(shell_path));
}

#[test]
// CodexBar: CodexExecutableResolverTests.swift:7
fn explicit_rpc_executable_survives_rejected_implicit_discovery() {
    let home = Path::new("/fakehome");
    let fs = MockFs::new(&["/usr/bin/true"]);
    let env = ProcessEnv::from_iter([("PATH", "/usr/bin:/bin"), ("SHELL", "/bin/sh")]);
    let inputs = ResolverInputs::new(&env, home, &fs);

    let resolution =
        resolve_codex_for_rpc(&inputs, "/usr/bin/true", || None).expect("explicit path succeeds");
    assert_eq!(resolution.executable.as_path(), Path::new("/usr/bin/true"));

    let resolution_implicit = resolve_codex_for_rpc(&inputs, "codex", || None);
    assert_eq!(resolution_implicit, Err(ResolveError::NotFound));
}

#[test]
// CodexBar: CodexExecutableResolverTests.swift:18
fn rejected_discovery_stops_rpc_before_process_launch() {
    let home = Path::new("/fakehome");
    let fs = MockFs::new(&[]);
    let env = ProcessEnv::from_iter([("PATH", "/synthetic/bin")]);
    let inputs = ResolverInputs::new(&env, home, &fs);

    let resolution = resolve_codex_for_rpc(&inputs, "codex", || {
        Some(vec![PathBuf::from("/synthetic/login/bin")])
    });
    assert_eq!(resolution, Err(ResolveError::NotFound));
}

#[test]
// CodexBar: CodexExecutableResolverTests.swift:45
fn explicit_native_override_skips_login_path_capture() {
    let home = Path::new("/fakehome");
    let fs = MockFs::new(&["/usr/bin/true"]);
    let env = ProcessEnv::from_iter([("CODEX_CLI_PATH", "/usr/bin/true")]);
    let inputs = ResolverInputs::new(&env, home, &fs);

    let capture_called = AtomicBool::new(false);
    let resolution = resolve_codex_for_rpc(&inputs, "codex", || {
        capture_called.store(true, Ordering::SeqCst);
        panic!("Native override should not capture a login-shell PATH");
    })
    .unwrap();

    assert!(!capture_called.load(Ordering::SeqCst));
    assert_eq!(resolution.executable.as_path(), Path::new("/usr/bin/true"));
    assert_eq!(resolution.login_path, None);
}

#[test]
// CodexBar: CodexExecutableResolverTests.swift:59
fn explicit_script_override_captures_login_path_for_env_based_launchers() {
    let home = Path::new("/fakehome");
    let script_path = "/fakehome/bin/codex-script";
    let fs = MockFs::new(&[]).with_script(script_path);
    let env = ProcessEnv::from_iter([("CODEX_CLI_PATH", script_path)]);
    let inputs = ResolverInputs::new(&env, home, &fs);

    let login_path = vec![PathBuf::from("/custom/node/bin"), PathBuf::from("/usr/bin")];
    let capture_count = std::sync::atomic::AtomicUsize::new(0);

    let expected_login = login_path.clone();
    let resolution = resolve_codex_for_rpc(&inputs, "codex", || {
        capture_count.fetch_add(1, Ordering::SeqCst);
        Some(login_path)
    })
    .unwrap();

    assert_eq!(resolution.executable.as_path(), Path::new(script_path));
    assert_eq!(resolution.login_path, Some(expected_login));
    assert_eq!(capture_count.load(Ordering::SeqCst), 1);
}

#[test]
fn mise_and_asdf_shims_found_after_codexbar_well_known_and_before_command_v() {
    let home = Path::new("/fakehome");
    let mise_shim = "/fakehome/.local/share/mise/shims/claude";
    let asdf_shim = "/fakehome/.asdf/shims/claude";
    let shell_path = "/shell/bin/claude";

    // 1) Test mise shim is found before command -v
    let fs_mise = MockFs::new(&[mise_shim, shell_path]);
    let env = ProcessEnv::from_iter([("SHELL", "/bin/zsh")]);
    let command_v_called = AtomicBool::new(false);
    let command_v = |_: &str| -> Option<PathBuf> {
        command_v_called.store(true, Ordering::SeqCst);
        Some(PathBuf::from(shell_path))
    };

    let inputs_mise = ResolverInputs::new(&env, home, &fs_mise).with_command_v(&command_v);
    let resolved_mise = resolve(Tool::Claude, &inputs_mise).unwrap();
    assert!(!command_v_called.load(Ordering::SeqCst));
    assert_eq!(resolved_mise.as_path(), Path::new(mise_shim));

    // 2) Test asdf shim is found before command -v when mise is absent
    let fs_asdf = MockFs::new(&[asdf_shim, shell_path]);
    let command_v_called_2 = AtomicBool::new(false);
    let command_v_2 = |_: &str| -> Option<PathBuf> {
        command_v_called_2.store(true, Ordering::SeqCst);
        Some(PathBuf::from(shell_path))
    };

    let inputs_asdf = ResolverInputs::new(&env, home, &fs_asdf).with_command_v(&command_v_2);
    let resolved_asdf = resolve(Tool::Claude, &inputs_asdf).unwrap();
    assert!(!command_v_called_2.load(Ordering::SeqCst));
    assert_eq!(resolved_asdf.as_path(), Path::new(asdf_shim));

    // 3) Test CodexBar well-known path (~/.local/bin/claude) is preferred over mise shim
    let well_known = "/fakehome/.local/bin/claude";
    let fs_pref = MockFs::new(&[well_known, mise_shim]);
    let inputs_pref = ResolverInputs::new(&env, home, &fs_pref);
    let resolved_pref = resolve(Tool::Claude, &inputs_pref).unwrap();
    assert_eq!(resolved_pref.as_path(), Path::new(well_known));
}
