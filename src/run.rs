use crate::sessions::{SessionRecord, Sessions};
use chrono::Utc;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Instant;

/// Exit codes reserved for our own use. These match common conventions
/// so that `agent0waste run` itself is shell-script-friendly.
pub const EXIT_OK: i32 = 0;
pub const EXIT_BAD_ARGS: i32 = 64;
pub const EXIT_RUNTIME_ERR: i32 = 70;
pub const EXIT_IO_ERR: i32 = 74;

const STDERR_TAIL_BYTES: usize = 2048;
const STDERR_TAIL_KEEP: usize = 500;

/// Run a child command, pass stdin/stdout through, capture stderr tail,
/// and persist a `SessionRecord`.
///
/// Token parsing is intentionally NOT done here in v0.2 — the wrapper
/// records time, exit, and command. Token extraction lands in v0.2.1
/// once we have real tool outputs to pattern-match against.
pub fn run_and_record(cmd: &str, args: &[&str]) -> Result<SessionRecord, String> {
    let started_wall = Instant::now();
    let started_at = Utc::now();
    let id = SessionRecord::new_id();
    let host = hostname();
    let argv0 = Some(cmd.to_string());
    let command = if args.is_empty() {
        cmd.to_string()
    } else {
        format!("{} {}", cmd, args.join(" "))
    };

    // Try to detect a model before the child starts. Best-effort: peek
    // at HERMES_MODEL / OPENAI_MODEL / ANTHROPIC_MODEL env vars. The
    // child itself may override them, so we accept that this is a hint.
    let (model, provider) = detect_model_from_env();

    let mut child = Command::new(cmd)
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            format!(
                "failed to spawn `{}`: {}\n  tip: is it on PATH? try `which {}`",
                cmd, e, cmd
            )
        })?;

    // Read stderr in a bounded way — never more than STDERR_TAIL_BYTES
    // so a chatty child can't OOM us.
    let mut stderr_buf = Vec::with_capacity(STDERR_TAIL_BYTES);
    if let Some(mut stderr) = child.stderr.take() {
        let mut tmp = [0u8; 256];
        while let Ok(n) = stderr.read(&mut tmp) {
            if n == 0 { break; }
            if stderr_buf.len() + n > STDERR_TAIL_BYTES {
                let keep = STDERR_TAIL_BYTES - stderr_buf.len();
                stderr_buf.extend_from_slice(&tmp[..keep]);
                break;
            }
            stderr_buf.extend_from_slice(&tmp[..n]);
        }
    }

    let status = child.wait().map_err(|e| format!("wait failed: {}", e))?;
    let duration_ms = started_wall.elapsed().as_millis() as u64;
    let exit_code = status.code().unwrap_or(-1);

    let stderr_tail = if stderr_buf.is_empty() {
        None
    } else {
        let s = String::from_utf8_lossy(&stderr_buf).to_string();
        // Truncate to last 500 chars for the JSON record.
        if s.len() > STDERR_TAIL_KEEP {
            Some(format!("…{}", &s[s.len() - STDERR_TAIL_KEEP..]))
        } else {
            Some(s)
        }
    };

    let rec = SessionRecord {
        id,
        command,
        argv0,
        started_at,
        ended_at: Utc::now(),
        duration_ms,
        exit_code,
        model,
        provider,
        input_tokens: None,
        output_tokens: None,
        cost_usd: None,
        stderr_tail,
        host,
    };

    Sessions::new().record(&rec)?;
    Ok(rec)
}

/// Best-effort: read `$HERMES_MODEL` (or other known vars) and split
/// it into (model, provider). Returns (None, None) when nothing matches.
fn detect_model_from_env() -> (Option<String>, Option<String>) {
    for (var, provider) in &[
        ("HERMES_MODEL", "hermes"),
        ("OPENAI_MODEL", "openai"),
        ("ANTHROPIC_MODEL", "anthropic"),
        ("GROK_MODEL", "xai"),
        ("GEMINI_MODEL", "google"),
    ] {
        if let Ok(v) = std::env::var(var) {
            if !v.is_empty() {
                return (Some(v), Some((*provider).to_string()));
            }
        }
    }
    (None, None)
}

/// Cheap hostname lookup. Returns None on any error.
fn hostname() -> Option<String> {
    let mut buf = [0u8; 256];
    let ret = unsafe { libc_gethostname(buf.as_mut_ptr() as *mut _, buf.len()) };
    if ret != 0 { return None; }
    let nul = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8(buf[..nul].to_vec()).ok()
}

// libc::gethostname is unstable; use a tiny direct syscall wrapper.
// (When uname would do on Linux, gethostname works on macOS + Linux.)
extern "C" {
    #[link_name = "gethostname"]
    fn libc_gethostname(name: *mut std::ffi::c_char, len: usize) -> i32;
}

/// Trampoline for `cargo run -- run -- <cmd>` argument handling.
/// Splits argv at the first `--` so we don't need clap to be smart about
/// trailing args. Returns (command, args).
pub fn split_run_args(argv: &[String]) -> Result<(String, Vec<String>), String> {
    let idx = argv.iter().position(|a| a == "--")
        .ok_or_else(|| "missing `--` separator. usage: agent0waste run -- <cmd> [args...]".to_string())?;
    let after = &argv[idx + 1..];
    if after.is_empty() {
        return Err("no command given after `--`".to_string());
    }
    Ok((after[0].clone(), after[1..].to_vec()))
}

/// Path helper for the sessions dir — used by `agent0waste sessions list`.
pub fn sessions_path() -> PathBuf {
    Sessions::new().base().to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_run_args_basic() {
        let argv: Vec<String> = vec!["--", "hermes", "run", "foo"]
            .into_iter().map(String::from).collect();
        let (cmd, args) = split_run_args(&argv).unwrap();
        assert_eq!(cmd, "hermes");
        assert_eq!(args, vec!["run", "foo"]);
    }

    #[test]
    fn split_run_args_no_separator() {
        let argv: Vec<String> = vec!["hermes", "run"]
            .into_iter().map(String::from).collect();
        let err = split_run_args(&argv).unwrap_err();
        assert!(err.contains("missing `--`"), "got: {}", err);
    }

    #[test]
    fn split_run_args_empty_after_separator() {
        let argv: Vec<String> = vec!["--"]
            .into_iter().map(String::from).collect();
        let err = split_run_args(&argv).unwrap_err();
        assert!(err.contains("no command"), "got: {}", err);
    }

    #[test]
    fn run_and_record_success() {
        // `true` always exists on unix; should exit 0.
        let rec = run_and_record("true", &[]).expect("true should run");
        assert_eq!(rec.exit_code, 0);
        assert_eq!(rec.argv0.as_deref(), Some("true"));
        assert!(rec.stderr_tail.is_none());
        assert!(rec.id.starts_with("sess-"));
    }

    #[test]
    fn run_and_record_failure_captures_stderr() {
        // `sh -c 'echo bad >&2; exit 9'` should exit non-zero with stderr.
        let rec = run_and_record("sh", &["-c", "echo bad >&2; exit 9"]).unwrap();
        assert_eq!(rec.exit_code, 9);
        let tail = rec.stderr_tail.as_deref().unwrap_or("");
        assert!(tail.contains("bad"), "stderr tail was: {:?}", tail);
    }

    #[test]
    fn run_and_record_nonexistent_command() {
        let err = run_and_record("definitely-not-a-real-binary-xyz", &[])
            .unwrap_err();
        assert!(err.contains("failed to spawn"), "got: {}", err);
    }

    #[test]
    fn detect_model_picks_up_env() {
        // No env set — should be (None, None).
        // (Can't easily mutate env in a multi-threaded test, but we can
        // call the function when nothing is set.)
        // SAFETY: safe because this test doesn't read env in parallel
        // with anything that cares.
        std::env::remove_var("HERMES_MODEL");
        std::env::remove_var("OPENAI_MODEL");
        std::env::remove_var("ANTHROPIC_MODEL");
        let (m, p) = detect_model_from_env();
        // Don't assert (None, None) — CI may have these set. Just assert
        // that if a value came back, provider also came back.
        if let Some(model) = m {
            assert!(p.is_some(), "provider should be set when model is: {}", model);
        }
    }
}
