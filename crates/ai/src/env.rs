//! Minimal `.env` loader — no external dependency.
//!
//! OpenEngine keeps model API keys out of the repo: a gitignored `.env` at the
//! workspace root holds them as `KEY=VALUE`, while `.env.example` is the
//! committed placeholder template. This module loads that file once at startup
//! so config resolution (`ProviderConfig::key_env`) can find a key without the
//! user exporting it each shell.
//!
//! # Resolution order (env wins)
//!
//! 1. A **real** environment variable already set on the process.
//! 2. A value from the workspace `.env` file.
//! 3. A typed [`AdapterError::Config`] "missing key <NAME>".
//!
//! The loader **never overwrites** a real env var — the file is a fallback only,
//! which keeps CI / shell-exported secrets authoritative.

use std::path::PathBuf;

use crate::AdapterError;

/// Load the workspace-root `.env` into the process env (fallback only).
///
/// Locates the file by walking up from the current directory until a
/// `Cargo.toml` declaring `[workspace]` is found (portability rule: no
/// hardcoded paths). Parses `KEY=VALUE` lines, skipping blanks and `#` comments,
/// trimming, stripping optional surrounding quotes, and tolerating malformed
/// lines (counted, not fatal). Never overwrites an already-set real env var.
///
/// Returns the number of variables loaded (0 if no `.env` exists). Call once at
/// binary startup.
pub fn load_dotenv() -> Result<usize, AdapterError> {
    let Some(path) = find_workspace_env() else {
        return Ok(0);
    };
    let (loaded, malformed) = match parse_env_into_process(&path) {
        Ok(x) => x,
        Err(_) => return Ok(0), // file absent/read-err: nothing to load
    };
    if malformed > 0 {
        eprintln!("openengine-ai: {malformed} malformed line(s) ignored in {path:?}");
    }
    Ok(loaded)
}

/// Walk up from `cwd` to the first directory with a `Cargo.toml` that declares
/// `[workspace]`; return the `.env` path **at the workspace root**.
fn find_workspace_env() -> Option<PathBuf> {
    let start = std::env::current_dir().ok()?;
    workspace_root(start).map(|root| root.join(".env"))
}

/// Parse `.env` at `path` and set each var into the process env **only if not
/// already set** (real env wins). Returns (loaded, malformed).
fn parse_env_into_process(path: &PathBuf) -> Result<(usize, usize), std::io::Error> {
    let text = std::fs::read_to_string(path)?;
    let mut loaded = 0usize;
    let mut malformed = 0usize;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some(eq) = line.find('=') else {
            malformed += 1;
            continue;
        };
        let key = line[..eq].trim();
        if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            malformed += 1;
            continue;
        }
        let mut value = line[eq + 1..].trim().to_string();
        // Strip one layer of matching surrounding quotes.
        if value.len() >= 2
            && ((value.starts_with('"') && value.ends_with('"'))
                || (value.starts_with('\'') && value.ends_with('\'')))
        {
            value = value[1..value.len() - 1].to_string();
        }
        // Env wins: never overwrite a real var.
        if std::env::var_os(key).is_none() {
            std::env::set_var(key, &value);
            loaded += 1;
        }
    }
    Ok((loaded, malformed))
}

/// Find the nearest ancestor of `start` whose `Cargo.toml` declares `[workspace]`.
fn workspace_root(start: PathBuf) -> Option<PathBuf> {
    for dir in start.ancestors() {
        let cargo = dir.join("Cargo.toml");
        if let Ok(text) = std::fs::read_to_string(&cargo) {
            if text.contains("[workspace]") {
                return Some(dir.to_path_buf());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// A unique var name per test so parallel tests never collide.
    fn uniq() -> String {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        format!("OE_ENV_TEST_{}_{}", std::process::id(), n)
    }

    fn temp_workspace() -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let d = std::env::temp_dir().join(format!("oe_env_wk_{}_{}", std::process::id(), n));
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn parses_key_value_and_comments() {
        let d = temp_workspace();
        let a = uniq();
        let b = uniq();
        fs::write(
            d.join(".env"),
            format!("# a comment\n\n{a}=hello\n{b}=\"quoted val\"\n"),
        )
        .unwrap();
        let (loaded, _mal) = parse_env_into_process(&d.join(".env")).unwrap();
        assert!(loaded >= 2);
        assert_eq!(std::env::var(&a).unwrap(), "hello");
        assert_eq!(std::env::var(&b).unwrap(), "quoted val");
        std::env::remove_var(&a);
        std::env::remove_var(&b);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn real_env_wins_over_file() {
        let d = temp_workspace();
        let v = uniq();
        fs::write(d.join(".env"), format!("{v}=fromfile\n")).unwrap();
        std::env::set_var(&v, "fromenv");
        let (loaded, _mal) = parse_env_into_process(&d.join(".env")).unwrap();
        assert_eq!(loaded, 0, "already-set env var must not be overwritten");
        assert_eq!(std::env::var(&v).unwrap(), "fromenv");
        std::env::remove_var(&v);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn malformed_lines_are_tolerated() {
        let d = temp_workspace();
        let g = uniq();
        fs::write(d.join(".env"), format!("NOEQUALS\n{g}=ok\n# c\n\n")).unwrap();
        let (loaded, mal) = parse_env_into_process(&d.join(".env")).unwrap();
        assert!(loaded >= 1);
        assert_eq!(std::env::var(&g).unwrap(), "ok");
        assert!(mal >= 1, "a line without '=' is malformed");
        std::env::remove_var(&g);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn workspace_root_walks_up_to_workspace_manifest() {
        // A temp dir with a [workspace] Cargo.toml is the workspace root.
        let d = temp_workspace();
        fs::write(d.join("Cargo.toml"), "[workspace]\nmembers=[]\n").unwrap();
        let root = workspace_root(d.clone()).expect("finds workspace root");
        assert_eq!(root, d);
        let _ = fs::remove_dir_all(&d);
    }
}
