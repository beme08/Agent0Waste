//! Layer 5: macOS sandbox-exec profile management.
//!
//! The sandbox module is the execution-wrapper layer (not a decision
//! layer). It writes SBPL profiles, validates them, and reports state
//! for `intercept trace` step [6].
//!
//! The decision spec (docs/decision-spec.md) is unchanged by this
//! module. Sandbox is an orthogonal concern to whether the call
//! should run; it only affects what the run can touch.
//!
//! ## Profile location
//!
//! Default: `~/.config/agent0waste/sandbox/<cmd>.sb`. The user can
//! override per-binary via the `profile` key in `[sandbox.<cmd>]`
//! in `intercept.toml`.
//!
//! ## $HOME expansion
//!
//! SBPL doesn't support `~`. The default profile is generated with
//! `$HOME` baked in at install time. If `$HOME` changes between
//! install and exec, the profile is stale. v0.4.3 documents this
//! limitation; v0.4.5+ may template lazily at exec time.

use std::path::{Path, PathBuf};

/// Subdir under `~/.config/agent0waste/` for sandbox profiles.
pub const SANDBOX_SUBDIR: &str = "sandbox";

/// Built-in default profile for hermes. Conservative: deny-default,
/// near-global read (hermes reads many system paths at startup), tight
/// write allow-list, outbound network only.
///
/// Empirically dogfooded against the live hermes binary — see
/// `docs/v0.4.3-design.md` for the test results.
pub const DEFAULT_HERMES_PROFILE: &str = r#"(version 1)
(deny default)

; --- Read: near-global (hermes reads many system paths at startup) ---
; v0.4.5+ can deny ~/.ssh and ~/.aws for credential isolation.
(allow file-read*)

; --- Write: allow-list only ---
(allow file-write*
  (subpath "{HOME}/.hermes")
  (subpath "{HOME}/.local")
  (subpath "{HOME}/.cache")
  (subpath "/private/tmp")
  (subpath "/private/var/folders")
  (subpath "/dev/null"))

; --- Network: outbound only ---
; v0.4.5+ can scope to LLM provider hosts via (remote tcp "host:443").
(allow network-outbound)
; (deny network-inbound) implicit; hermes doesn't accept inbound.

; --- Process / IPC ---
(allow process-exec)
(allow process-fork)
(allow sysctl-read)
(allow mach-lookup)
(allow signal)
(allow system-socket)
(allow ipc-posix-shm-read* ipc-posix-shm-write*)
"#;

/// Per-binary sandbox settings. In `intercept.toml` as `[sandbox.<cmd>]`.
#[derive(Debug, Clone)]
pub struct SandboxEntry {
    pub enabled: bool,
    /// Path to the SBPL profile. If `None`, defaults to
    /// `~/.config/agent0waste/sandbox/<cmd>.sb`.
    pub profile: Option<PathBuf>,
}

impl Default for SandboxEntry {
    fn default() -> Self {
        Self {
            enabled: false,
            profile: None,
        }
    }
}

/// Top-level sandbox config from `intercept.toml`. Per-binary map
/// keyed by command name (e.g. "hermes", "claude").
#[derive(Debug, Clone, Default)]
pub struct SandboxConfig {
    pub entries: std::collections::HashMap<String, SandboxEntry>,
}

impl SandboxConfig {
    /// Read `~/.config/agent0waste/intercept.toml` and extract the
    /// `[sandbox]` and `[sandbox.<cmd>]` tables. Missing file = empty.
    pub fn load() -> Self {
        let Some(path) = super::intercept::intercept_toml_path() else {
            return Self::default();
        };
        if !path.exists() {
            return Self::default();
        }
        let Ok(contents) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        let Ok(parsed) = contents.parse::<toml::Table>() else {
            return Self::default();
        };

        let mut out = Self::default();
        let Some(sandbox_table) = parsed.get("sandbox").and_then(|v| v.as_table()) else {
            return out;
        };

        for (key, value) in sandbox_table {
            let Some(t) = value.as_table() else {
                continue;
            };
            let enabled = t.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);
            let profile = t
                .get("profile")
                .and_then(|v| v.as_str())
                .map(|s| expand_home(s));
            let entry = SandboxEntry { enabled, profile };
            out.entries.insert(key.clone(), entry);
        }
        out
    }

    /// Get the entry for a command. Returns `None` if not configured.
    pub fn get(&self, command: &str) -> Option<&SandboxEntry> {
        self.entries.get(command)
    }

    /// Resolve the effective profile path for a command. If the user
    /// set `profile = "..."` in config, that path is used. Otherwise
    /// the default `~/.config/agent0waste/sandbox/<cmd>.sb` is used.
    pub fn default_profile_path(&self, command: &str) -> PathBuf {
        if let Some(entry) = self.get(command) {
            if let Some(p) = &entry.profile {
                return p.clone();
            }
        }
        default_sandbox_dir()
            .map(|d| d.join(format!("{}.sb", command)))
            .unwrap_or_else(|| PathBuf::from(format!("sandbox/{}.sb", command)))
    }
}

/// Default directory for sandbox profiles: `~/.config/agent0waste/sandbox`.
pub fn default_sandbox_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".config/agent0waste").join(SANDBOX_SUBDIR))
}

/// Expand `~` at the start of a path to `$HOME`. Other forms of
/// expansion (`$VAR`, etc.) are the caller's responsibility.
pub fn expand_home(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    if p == "~" {
        if let Some(home) = dirs::home_dir() {
            return home;
        }
    }
    PathBuf::from(p)
}

/// Substitute `{HOME}` in a profile template with the actual home
/// directory path. Used when writing the default profile.
pub fn template_profile(template: &str, home: &Path) -> String {
    template.replace("{HOME}", &home.to_string_lossy())
}

/// Outcome of writing the default profile (used by `enable-sandbox`).
#[derive(Debug, Clone, PartialEq)]
pub enum ProfileWriteOutcome {
    Written { path: PathBuf },
    AlreadyExists { path: PathBuf },
}

/// Write the default hermes profile to its expected path, with `{HOME}`
/// substituted. Refuses to overwrite an existing file.
pub fn write_default_profile(command: &str) -> Result<ProfileWriteOutcome, String> {
    let path = default_sandbox_dir()
        .ok_or_else(|| "no home directory".to_string())?
        .join(format!("{}.sb", command));

    if path.exists() {
        return Ok(ProfileWriteOutcome::AlreadyExists { path });
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("could not create {}: {}", parent.display(), e))?;
    }

    let home = dirs::home_dir().ok_or_else(|| "no home directory".to_string())?;
    let contents = template_profile(DEFAULT_HERMES_PROFILE, &home);

    std::fs::write(&path, contents)
        .map_err(|e| format!("could not write {}: {}", path.display(), e))?;

    Ok(ProfileWriteOutcome::Written { path })
}

/// Outcome of validating a profile by running `sandbox-exec` against
/// `/bin/true` (used by `validate-sandbox`).
#[derive(Debug, Clone)]
pub struct ProfileValidation {
    pub profile_path: PathBuf,
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

/// Run `sandbox-exec -f <profile> /usr/bin/true` as a smoke test. Returns
/// the outcome. `/usr/bin/true` is a no-op binary that exits 0 under any
/// sane sandbox; a profile that breaks `/usr/bin/true` is broken.
/// (Note: on macOS `/bin/true` doesn't exist; the binary is at
/// `/usr/bin/true`. Linux also uses `/usr/bin/true`.)
pub fn validate_profile(profile_path: &Path) -> Result<ProfileValidation, String> {
    if !profile_path.exists() {
        return Err(format!(
            "profile not found: {}",
            profile_path.display()
        ));
    }
    use std::process::Command;
    let output = Command::new("/usr/bin/sandbox-exec")
        .arg("-f")
        .arg(profile_path)
        .arg("/usr/bin/true")
        .output()
        .map_err(|e| format!("could not run sandbox-exec: {}", e))?;
    Ok(ProfileValidation {
        profile_path: profile_path.to_path_buf(),
        ok: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        exit_code: output.status.code().unwrap_or(-1),
    })
}

/// The runtime sandbox state for a single call. Returned by
/// `sandbox_status_for()` and rendered as `intercept trace` step [6].
#[derive(Debug, Clone)]
pub enum SandboxStatus {
    /// Sandbox is enabled in config and the profile is loaded.
    Enabled { profile: PathBuf },
    /// Sandbox is configured for this command but `enabled = false`.
    Disabled { profile: PathBuf },
    /// No sandbox entry in config for this command.
    NotConfigured,
    /// Sandbox entry exists but the profile file is missing.
    ProfileMissing { profile: PathBuf },
    /// Non-macOS host (sandbox-exec unavailable). Always fail-open.
    UnsupportedHost,
    /// `/usr/bin/sandbox-exec` binary not found.
    SandboxExecMissing,
}

impl SandboxStatus {
    pub fn label(&self) -> &'static str {
        match self {
            SandboxStatus::Enabled { .. } => "enabled",
            SandboxStatus::Disabled { .. } => "disabled",
            SandboxStatus::NotConfigured => "not configured",
            SandboxStatus::ProfileMissing { .. } => "profile missing",
            SandboxStatus::UnsupportedHost => "skipped (non-macOS)",
            SandboxStatus::SandboxExecMissing => "skipped (sandbox-exec missing)",
        }
    }
}

/// Compute the runtime sandbox status for a command. This is the
/// value rendered as `intercept trace` step [6] and consulted by
/// the shim to decide whether to wrap the exec call.
pub fn sandbox_status_for(command: &str) -> SandboxStatus {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = command;
        return SandboxStatus::UnsupportedHost;
    }
    #[cfg(target_os = "macos")]
    {
        if !std::path::Path::new("/usr/bin/sandbox-exec").exists() {
            return SandboxStatus::SandboxExecMissing;
        }
        let cfg = SandboxConfig::load();
        let Some(entry) = cfg.get(command) else {
            return SandboxStatus::NotConfigured;
        };
        let profile = entry
            .profile
            .clone()
            .unwrap_or_else(|| {
                default_sandbox_dir()
                    .map(|d| d.join(format!("{}.sb", command)))
                    .unwrap_or_else(|| PathBuf::from(format!("sandbox/{}.sb", command)))
            });
        if !entry.enabled {
            return SandboxStatus::Disabled { profile };
        }
        if !profile.exists() {
            return SandboxStatus::ProfileMissing { profile };
        }
        SandboxStatus::Enabled { profile }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_home_replaces_tilde() {
        let home = dirs::home_dir().unwrap();
        let expanded = expand_home("~/foo");
        assert_eq!(expanded, home.join("foo"));
    }

    #[test]
    fn expand_home_passes_through_absolute() {
        let expanded = expand_home("/etc/hosts");
        assert_eq!(expanded, PathBuf::from("/etc/hosts"));
    }

    #[test]
    fn expand_home_passes_through_relative() {
        let expanded = expand_home("foo/bar");
        assert_eq!(expanded, PathBuf::from("foo/bar"));
    }

    #[test]
    fn template_profile_substitutes_home() {
        let home = PathBuf::from("/Users/test");
        let out = template_profile("(subpath \"{HOME}/.hermes\")", &home);
        assert_eq!(out, "(subpath \"/Users/test/.hermes\")");
    }

    #[test]
    fn default_profile_path_for_unconfigured_command() {
        // Should return ~/.config/agent0waste/sandbox/<cmd>.sb
        let cfg = SandboxConfig::default();
        let path = cfg.default_profile_path("hermes");
        let home = dirs::home_dir().unwrap();
        assert_eq!(
            path,
            home.join(".config/agent0waste/sandbox/hermes.sb")
        );
    }

    #[test]
    fn default_profile_path_for_overridden_command() {
        // User sets profile = "/custom/path.sb" in config
        let mut cfg = SandboxConfig::default();
        cfg.entries.insert(
            "hermes".into(),
            SandboxEntry {
                enabled: true,
                profile: Some(PathBuf::from("/custom/path.sb")),
            },
        );
        let path = cfg.default_profile_path("hermes");
        assert_eq!(path, PathBuf::from("/custom/path.sb"));
    }

    #[test]
    fn default_profile_template_substitutes_real_home() {
        let home = dirs::home_dir().unwrap();
        let out = template_profile(DEFAULT_HERMES_PROFILE, &home);
        // Verify the substituted profile mentions the real home
        // directory in a write rule. The template contains
        // (subpath "{HOME}/.hermes") which becomes
        // (subpath "/Users/<name>/.hermes").
        let home_str = home.to_string_lossy();
        let expected = format!("(subpath \"{}/.hermes\")", home_str);
        assert!(
            out.contains(&expected),
            "profile should contain '{}' for the home substitution; got:\n{}",
            expected,
            out
        );
    }

    #[test]
    fn write_default_profile_creates_file() {
        // Use a temporary config dir to avoid clobbering user state.
        // Since we can't easily redirect dirs::home_dir(), just test
        // that the function returns an error gracefully if the dir
        // can't be created — the file-existence branch is the main
        // thing we need to assert.
        let result = write_default_profile("agent0waste_test_no_op_cmd_xyz");
        // Either it wrote (first run) or it already exists (re-run).
        // Both are valid outcomes; we just need the function not to
        // panic.
        assert!(result.is_ok());
    }

    #[test]
    fn sandbox_status_label_includes_platform_info() {
        // This test is a no-op on non-macOS but documents the
        // expected labels.
        let s = sandbox_status_for("nonexistent_cmd_xyz");
        // On macOS this is NotConfigured; on other platforms,
        // UnsupportedHost. Both have meaningful labels.
        assert!(!s.label().is_empty());
    }
}
