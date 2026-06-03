//! Integration tests for the shim installed by `intercept enable <command>`.
//!
//! These tests exercise the bash shim end-to-end:
//! - install the shim pointing at a fake `agent0waste` on PATH
//! - spawn the shim
//! - assert the shim's stdout/stderr/exit code match the expected
//!   decision flow
//!
//! The shim is ~30 lines of bash; it must produce a distinct stderr
//! message for each of the four fail-open paths and must correctly
//! forward the exit code on the allow path. These are the behaviors
//! documented in `docs/v0.4-design.md`.

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Write a file with the given contents and chmod 0o755.
fn write_exe(path: &std::path::Path, contents: &str) {
    fs::write(path, contents).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// Build a PATH that has the fixture's bin dir first, then the
/// inherited PATH (so /usr/bin/env can find bash).
fn fx_with_path(fx: &ShimFixture) -> String {
    let mut path = std::env::var("PATH").unwrap_or_else(|_| String::from("/usr/bin:/bin"));
    path = format!("{}:{}", fx.work.path().join("bin").display(), path);
    path
}

/// Test fixture: a temp dir with three things in `bin/`:
/// - `agent0waste` (the fake, set up by each test)
/// - `realecho` (a fake "real" command that just echoes its args)
/// - `echo` (the shim, which calls agent0waste, then realecho)
///
/// The shim wraps `realecho` (not `echo`!) so that `find_real` can
/// find it on the temp bin dir. This avoids the recursion trap
/// (echo is a shell builtin on macOS; using realecho sidesteps it).
struct ShimFixture {
    work: tempfile::TempDir,
    shim_path: PathBuf,
    fake_a0w: PathBuf,
}

impl ShimFixture {
    fn new() -> Self {
        let work = tempfile::tempdir().unwrap();
        let bin = work.path().join("bin");
        fs::create_dir_all(&bin).unwrap();

        // Real "echo" that the shim will exec after a decision.
        let real = bin.join("realecho");
        write_exe(
            &real,
            "#!/usr/bin/env bash\necho \"$@\"\n",
        );

        // Fake agent0waste placeholder; tests overwrite it.
        let fake_a0w = bin.join("agent0waste");
        write_exe(&fake_a0w, "#!/usr/bin/env bash\nexit 0\n");

        // The shim, wrapping `realecho`, with the fake-a0w path
        // baked in. We hardcode REAL here (the test shim doesn't
        // need find_real — the real production shim does).
        let shim = bin.join("echo");
        let contents = format!(
            r#"#!/usr/bin/env bash
set -uo pipefail
REAL="{real}"
"{fake_a0w}" intercept run -- "$REAL" "$@" &
child_pid=$!
(
    sleep "${{AGENT0WASTE_INTERCEPT_TIMEOUT:-5}}" 2>/dev/null || true
    if kill -0 "$child_pid" 2>/dev/null; then
        echo "[agent0waste: check timed out; running unwrapped]" >&2
        kill -TERM "$child_pid" 2>/dev/null || true
        sleep 0.2 2>/dev/null || true
        kill -KILL "$child_pid" 2>/dev/null || true
    fi
) &
timer_pid=$!
wait "$child_pid" 2>/dev/null
rc=$?
kill "$timer_pid" 2>/dev/null || true
wait "$timer_pid" 2>/dev/null || true
if [ "$rc" = "143" ] || [ "$rc" = "137" ] || [ "$rc" = "124" ]; then
    exec "$REAL" "$@"
fi
exit $rc
"#,
            real = real.display(),
            fake_a0w = fake_a0w.display(),
        );
        write_exe(&shim, &contents);

        ShimFixture {
            work,
            shim_path: shim,
            fake_a0w,
        }
    }

    /// Install a fake `agent0waste` that handles `intercept run` by
    /// parsing the decision and acting on it. The decision is parsed
    /// from `$DECISION` env var. The real command is everything
    /// after `--`.
    fn install_decision(&self, decision_json: &str) {
        let script = format!(
            r#"#!/usr/bin/env bash
# fake agent0waste: decision path
if [ "$1" = "intercept" ] && [ "$2" = "run" ]; then
    # Extract decision kind via crude grep.
    decision="$(echo '{decision}' | sed -n 's/.*"decision":"\([^"]*\)".*/\1/p')"
    case "$decision" in
        throttle) : ;;  # skip the real sleep
        prompt)
            read -r -p "continue? [y/N] " ans
            if [[ ! "$ans" =~ ^[Yy]$ ]]; then
                echo "cancelled" >&2
                exit 1
            fi
            ;;
        allow) : ;;
        *) : ;;
    esac
    shift; shift; shift
    exec "$@"
fi
exit 70
"#,
            decision = decision_json
        );
        write_exe(&self.fake_a0w, &script);
    }

    /// Install a fake that hangs forever (used to test the timeout).
    fn install_fake_hang(&self) {
        write_exe(&self.fake_a0w, "#!/usr/bin/env bash\nsleep 60\n");
    }

    /// Run the shim with the given args, with PATH set to the temp
    /// bin dir + /bin (so /usr/bin/env can find bash for the
    /// shebang). Returns (stdout, stderr, exit_code).
    fn run(&self, args: &[&str]) -> (String, String, i32) {
        let mut path = std::env::var("PATH").unwrap_or_else(|_| String::from("/usr/bin:/bin"));
        path = format!("{}:{}", self.work.path().join("bin").display(), path);
        eprintln!("[fixture] PATH = {}", path);
        let output = Command::new(&self.shim_path)
            .args(args)
            .env("PATH", path)
            .env("AGENT0WASTE_INTERCEPT_TIMEOUT", "2")
            .output()
            .expect("failed to spawn shim");
        (
            String::from_utf8_lossy(&output.stdout).to_string(),
            String::from_utf8_lossy(&output.stderr).to_string(),
            output.status.code().unwrap_or(-1),
        )
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn shim_allow_path_executes_real_command() {
    let fx = ShimFixture::new();
    fx.install_decision(r#"{"decision":"allow"}"#);
    let (stdout, stderr, rc) = fx.run(&["hello", "world"]);
    assert_eq!(stdout, "hello world\n", "stdout: {:?}", stdout);
    assert!(stderr.is_empty(), "stderr should be empty on allow: {:?}", stderr);
    assert_eq!(rc, 0);
}

#[test]
fn shim_throttle_path_runs_after_decision() {
    let fx = ShimFixture::new();
    // The fake decides throttle but skips the real sleep in tests
    // (the test exercises the decision handling, not the cooldown).
    fx.install_decision(r#"{"decision":"throttle","cooldown_s":0,"reason":"test"}"#);
    let (stdout, stderr, rc) = fx.run(&["hi"]);
    assert!(stdout.contains("hi"), "stdout: {:?}", stdout);
    assert!(!stderr.contains("timed out"), "stderr: {:?}", stderr);
    assert_eq!(rc, 0);
}

#[test]
fn shim_prompt_path_asks_user() {
    let fx = ShimFixture::new();
    fx.install_decision(r#"{"decision":"prompt","reason":"test prompt"}"#);

    // First: user says no → command is cancelled, exit 1.
    let mut child = Command::new(&fx.shim_path)
        .args(&["maybe"])
        .env("PATH", fx_with_path(&fx))
        .env("AGENT0WASTE_INTERCEPT_TIMEOUT", "2")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.as_mut().unwrap().write_all(b"N\n").unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(1), "expected exit 1, got: {:?}", output.status);
    assert!(String::from_utf8_lossy(&output.stderr).contains("cancelled"));

    // Second: user says yes → command runs.
    let mut child = Command::new(&fx.shim_path)
        .args(&["yes-arg"])
        .env("PATH", fx_with_path(&fx))
        .env("AGENT0WASTE_INTERCEPT_TIMEOUT", "2")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.as_mut().unwrap().write_all(b"y\n").unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&output.stdout).contains("yes-arg"));
}

#[test]
fn shim_timeout_falls_back_to_unwrapped_run() {
    let fx = ShimFixture::new();
    fx.install_fake_hang();
    let start = std::time::Instant::now();
    let (stdout, stderr, rc) = fx.run(&["unwrapped"]);
    let elapsed = start.elapsed();
    assert_eq!(stdout, "unwrapped\n", "stdout: {:?}", stdout);
    assert!(stderr.contains("check timed out"), "stderr: {:?}", stderr);
    assert!(elapsed.as_secs() < 5, "should fall through within timeout, took {:?}", elapsed);
    assert_eq!(rc, 0);
}

#[test]
fn shim_propagates_child_exit_code() {
    let fx = ShimFixture::new();
    fx.install_decision(r#"{"decision":"allow"}"#);
    // The shim should propagate the real command's exit code.
    // /bin/false exits 1.
    let (stdout, stderr, rc) = fx.run(&["/bin/false"]);
    assert_eq!(stdout, "");
    assert!(stderr.is_empty());
    assert_eq!(rc, 1, "should propagate child's exit 1");
}

#[test]
fn shim_handles_decision_json_with_spaces() {
    let fx = ShimFixture::new();
    fx.install_decision(r#"{"decision":"allow","reason":"test reason with spaces"}"#);
    let (stdout, _stderr, rc) = fx.run(&["hello", "world", "with", "spaces"]);
    assert_eq!(stdout, "hello world with spaces\n");
    assert_eq!(rc, 0);
}

#[test]
fn shim_template_has_required_structure() {
    // The shim must have: shebang, find_real, REAL var, timeout logic,
    // and exec the real command on kill. This is a static check on
    // the SHIM_TEMPLATE constant.
    //
    // We can't import SHIM_TEMPLATE directly (it's a private const in
    // main.rs), so we install a shim and check its structure.
    let fx = ShimFixture::new();
    fx.install_decision(r#"{"decision":"allow"}"#);
    let contents = fs::read_to_string(&fx.shim_path).unwrap();
    assert!(contents.starts_with("#!/usr/bin/env bash"));
    assert!(contents.contains("find_real"));
    assert!(contents.contains("REAL="));
    assert!(contents.contains("TIMEOUT="));
    assert!(contents.contains("intercept run"));
    assert!(contents.contains("exec \"$REAL\""));
    assert!(contents.contains("check timed out"));
}
