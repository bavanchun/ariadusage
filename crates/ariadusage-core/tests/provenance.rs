use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

#[derive(Debug, Deserialize)]
struct Manifest {
    baseline: String,
    #[serde(default)]
    fixture: Vec<FixtureEntry>,
    #[serde(default)]
    ported: Vec<PortedEntry>,
}

#[derive(Debug, Deserialize)]
struct FixtureEntry {
    path: String,
    origin: String,
    source: Option<String>,
    commit: Option<String>,
    license: String,
    changes: String,
}

#[derive(Debug, Deserialize)]
struct PortedEntry {
    path: String,
    sources: Vec<String>,
}

fn header_needle() -> String {
    ["Ported", "from", "CodexBar"].join(" ")
}

fn header_prefix() -> String {
    format!("// {}", header_needle())
}

fn check_provenance(workspace_root: &Path) -> Result<(), String> {
    let manifest_path = workspace_root.join("fixtures/manifest.toml");
    if !manifest_path.exists() {
        return Err(format!("Manifest not found at {}", manifest_path.display()));
    }

    let manifest_text =
        fs::read_to_string(&manifest_path).map_err(|e| format!("Failed to read manifest: {e}"))?;
    let manifest: Manifest =
        toml::from_str(&manifest_text).map_err(|e| format!("Failed to parse manifest: {e}"))?;

    if manifest.baseline != "6a26b2e9b1b60471970deb6fe663f9e5f284e2ce" {
        return Err(format!(
            "Manifest baseline commit must be 6a26b2e9b1b60471970deb6fe663f9e5f284e2ce, got {}",
            manifest.baseline
        ));
    }

    // Check fixtures
    let fixtures_dir = workspace_root.join("fixtures");
    let mut actual_fixtures = HashSet::new();
    if fixtures_dir.exists() {
        let mut stack = vec![fixtures_dir.clone()];
        while let Some(dir) = stack.pop() {
            let entries = fs::read_dir(&dir)
                .map_err(|e| format!("Failed to read dir {}: {e}", dir.display()))?;
            for entry in entries {
                let entry = entry.map_err(|e| e.to_string())?;
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.is_file() {
                    let rel = path
                        .strip_prefix(&fixtures_dir)
                        .map_err(|e| e.to_string())?
                        .to_string_lossy()
                        .to_string();
                    if rel != "manifest.toml" {
                        actual_fixtures.insert(rel);
                    }
                }
            }
        }
    }

    let mut declared_fixtures = HashSet::new();
    for fix in &manifest.fixture {
        if !declared_fixtures.insert(&fix.path) {
            return Err(format!("Duplicate fixture entry in manifest: {}", fix.path));
        }

        let fix_path = fixtures_dir.join(&fix.path);
        if !fix_path.exists() {
            return Err(format!(
                "Fixture declared in manifest does not exist on disk: {}",
                fix.path
            ));
        }

        match fix.origin.as_str() {
            "codexbar" => {
                let source = fix
                    .source
                    .as_deref()
                    .ok_or_else(|| format!("codexbar fixture {} missing source", fix.path))?;
                if source.trim().is_empty() {
                    return Err(format!("codexbar fixture {} has empty source", fix.path));
                }
                let commit = fix
                    .commit
                    .as_deref()
                    .ok_or_else(|| format!("codexbar fixture {} missing commit", fix.path))?;
                if commit != manifest.baseline {
                    return Err(format!(
                        "codexbar fixture {} commit does not match baseline",
                        fix.path
                    ));
                }
                if fix.license != "MIT" {
                    return Err(format!(
                        "codexbar fixture {} license must be MIT, got {}",
                        fix.path, fix.license
                    ));
                }
                if fix.changes.trim().is_empty() {
                    return Err(format!("codexbar fixture {} has empty changes", fix.path));
                }
            }
            "ariadusage" => {
                if fix.source.is_some() {
                    return Err(format!(
                        "ariadusage fixture {} must not declare a source",
                        fix.path
                    ));
                }
                if fix.license.trim().is_empty() {
                    return Err(format!("ariadusage fixture {} has empty license", fix.path));
                }
                if fix.changes.trim().is_empty() {
                    return Err(format!("ariadusage fixture {} has empty changes", fix.path));
                }
            }
            other => {
                return Err(format!("Unknown fixture origin in manifest: {other}"));
            }
        }
    }

    for actual in &actual_fixtures {
        if !declared_fixtures.contains(actual) {
            return Err(format!(
                "File in fixtures/ not declared in manifest: {actual}"
            ));
        }
    }

    // Check ported files
    let needle = header_needle();
    let expected_prefix = header_prefix();
    let expected_suffix = "; MIT, see LICENSES/CodexBar-MIT.txt";

    let mut ported_map = HashMap::new();
    for p in &manifest.ported {
        if ported_map
            .insert(p.path.clone(), p.sources.clone())
            .is_some()
        {
            return Err(format!(
                "Duplicate [[ported]] entry in manifest: {}",
                p.path
            ));
        }
        let full_path = workspace_root.join(&p.path);
        if !full_path.exists() {
            return Err(format!(
                "Ported file declared in manifest does not exist: {}",
                p.path
            ));
        }
    }

    // Walk all crates/**/*.rs
    let crates_dir = workspace_root.join("crates");
    let mut rs_files = Vec::new();
    let mut stack = vec![crates_dir];
    while let Some(dir) = stack.pop() {
        let entries =
            fs::read_dir(&dir).map_err(|e| format!("Failed to read dir {}: {e}", dir.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("rs") {
                let rel = path
                    .strip_prefix(workspace_root)
                    .map_err(|e| e.to_string())?
                    .to_string_lossy()
                    .to_string();
                rs_files.push((rel, path));
            }
        }
    }

    for (rel_path, abs_path) in rs_files {
        let content = fs::read_to_string(&abs_path)
            .map_err(|e| format!("Failed to read {}: {e}", abs_path.display()))?;
        let lines: Vec<&str> = content.lines().collect();

        // Identify leading comment block
        let mut leading_comments = Vec::new();
        let mut in_leading = true;
        for line in &lines {
            let trimmed = line.trim();
            if in_leading {
                if trimmed.is_empty() || trimmed.starts_with("//") {
                    leading_comments.push(*line);
                } else {
                    in_leading = false;
                }
            }
        }

        let is_ported = ported_map.contains_key(&rel_path);

        if !is_ported {
            if content.contains(&needle) {
                return Err(format!(
                    "File outside [[ported]] contains needle ({}): {}",
                    needle, rel_path
                ));
            }
        } else {
            let expected_sources = ported_map.get(&rel_path).unwrap();
            let mut found_sources = Vec::new();

            for line in &leading_comments {
                let trimmed = line.trim();
                if trimmed.starts_with(&expected_prefix) {
                    if !trimmed.ends_with(expected_suffix) {
                        return Err(format!(
                            "Header line in {} does not end with expected suffix '{}': {}",
                            rel_path, expected_suffix, trimmed
                        ));
                    }
                    let without_prefix = trimmed.strip_prefix(&expected_prefix).unwrap().trim();
                    let without_suffix =
                        without_prefix.strip_suffix(expected_suffix).unwrap().trim();
                    let parts: Vec<&str> = without_suffix.split_whitespace().collect();
                    if parts.len() != 3 || parts[1] != "at" || parts[2] != "6a26b2e9b" {
                        return Err(format!(
                            "Header line in {} malformed format: {}",
                            rel_path, trimmed
                        ));
                    }
                    found_sources.push(parts[0].to_string());
                }
            }

            if found_sources.len() != expected_sources.len() {
                return Err(format!(
                    "Ported file {} expected {} headers, found {}: {:?}",
                    rel_path,
                    expected_sources.len(),
                    found_sources.len(),
                    found_sources
                ));
            }

            for expected in expected_sources {
                if !found_sources.contains(expected) {
                    return Err(format!(
                        "Ported file {} missing header for source: {}",
                        rel_path, expected
                    ));
                }
            }

            // Verify no needle outside the leading comment block or prose mentions
            let mut leading_comment_lines = 0;
            for line in &lines {
                let trimmed = line.trim();
                if trimmed.is_empty() || trimmed.starts_with("//") {
                    leading_comment_lines += 1;
                } else {
                    break;
                }
            }

            for (idx, line) in lines.iter().enumerate() {
                if line.contains(&needle) {
                    if idx >= leading_comment_lines {
                        return Err(format!(
                            "Header needle found outside leading comment block in {} at line {}: {}",
                            rel_path,
                            idx + 1,
                            line
                        ));
                    }
                    let trimmed = line.trim();
                    if !trimmed.starts_with(&expected_prefix) || !trimmed.ends_with(expected_suffix)
                    {
                        return Err(format!(
                            "Prose mention of header needle in leading comments in {} at line {}: {}",
                            rel_path,
                            idx + 1,
                            line
                        ));
                    }
                }
            }
        }
    }

    Ok(())
}

#[test]
fn test_provenance() {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("parent crates")
        .parent()
        .expect("workspace root");

    let result = check_provenance(workspace_root);
    assert!(
        result.is_ok(),
        "Provenance check failed: {}",
        result.unwrap_err()
    );
}

#[test]
fn test_provenance_negative_planted_midfile_and_missing_header() {
    let temp_dir = std::env::temp_dir().join(format!("provenance_neg_{}", std::process::id()));
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir).unwrap();

    let fixtures_dir = temp_dir.join("fixtures");
    fs::create_dir_all(&fixtures_dir).unwrap();
    let manifest_content = r#"
baseline = "6a26b2e9b1b60471970deb6fe663f9e5f284e2ce"

[[ported]]
path = "crates/test/src/lib.rs"
sources = ["Sources/Test.swift"]
"#;
    fs::write(fixtures_dir.join("manifest.toml"), manifest_content).unwrap();

    let crates_dir = temp_dir.join("crates/test/src");
    fs::create_dir_all(&crates_dir).unwrap();

    // 1. Missing header
    fs::write(crates_dir.join("lib.rs"), "pub fn hello() {}\n").unwrap();
    let err = check_provenance(&temp_dir).unwrap_err();
    assert!(
        err.contains("expected 1 headers, found 0") || err.contains("missing header"),
        "Unexpected error: {err}"
    );

    // 2. Planted mid-file mention
    let needle = header_needle();
    let valid_header = format!(
        "// {} Sources/Test.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt\n",
        needle
    );
    let planted = format!(
        "{}pub fn hello() {{\n    // {} here\n}}\n",
        valid_header, needle
    );
    fs::write(crates_dir.join("lib.rs"), planted).unwrap();
    let err2 = check_provenance(&temp_dir).unwrap_err();
    assert!(
        err2.contains("outside leading comment block") || err2.contains("Prose mention"),
        "Unexpected error: {err2}"
    );

    let _ = fs::remove_dir_all(&temp_dir);
}
