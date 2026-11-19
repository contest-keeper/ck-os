//! The password of the `ckadmin` account (decision 3: sudo only with a per-contest password).
//!
//! The image ships the account with a locked password. When the settings carry
//! `admin_password_hash`, the agent gives it to `chpasswd -e` on standard input at every boot. The
//! hash is never an argument (it would show in the process list) and never a log line.

use std::io::{self, Write};
use std::path::Path;
use std::process::{Command, Stdio};

/// The account the image creates, in the `sudo` group.
const ACCOUNT: &str = "ckadmin";

/// What `set_password` did.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The settings carry no hash, so nothing was run and the account keeps the image's locked state.
    Locked,
    /// `chpasswd` accepted the hash.
    Set,
}

/// The one line `chpasswd -e` reads. The hash is validated by ck-protocol (printable ASCII, no
/// colon); this refuses anything that could add a second line or field anyway.
fn input_line(hash: &str) -> Option<String> {
    let safe = !hash.is_empty() && hash.chars().all(|c| c.is_ascii_graphic() && c != ':');
    safe.then(|| format!("{ACCOUNT}:{hash}\n"))
}

/// Sets the password of `ckadmin` from `hash` with the program `chpasswd` (a path, so a test can
/// use a stand-in). Errors never contain the hash.
pub fn set_password(chpasswd: &Path, hash: Option<&str>) -> io::Result<Outcome> {
    let Some(hash) = hash else {
        return Ok(Outcome::Locked);
    };
    let line = input_line(hash).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "the admin password hash is not usable",
        )
    })?;

    let mut child = Command::new(chpasswd)
        .arg("-e")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()?;
    let mut stdin = child.stdin.take().expect("stdin was requested as a pipe");
    let written = stdin.write_all(line.as_bytes());
    // Closing stdin lets chpasswd finish.
    drop(stdin);
    let status = child.wait()?;
    written?;
    if status.success() {
        Ok(Outcome::Set)
    } else {
        Err(io::Error::other(format!("chpasswd failed ({status})")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HASH: &str = "$6$salt$abcdefghijklmnopqrstuvwxyz0123456789";

    #[test]
    fn the_line_names_the_account_and_ends_with_one_newline() {
        assert_eq!(input_line(HASH), Some(format!("ckadmin:{HASH}\n")));
    }

    #[test]
    fn a_hash_that_could_add_a_line_or_a_field_is_refused() {
        for bad in ["", "$6$a\n$b", "$6$a:b", "$6$a b", "$6$a\r"] {
            assert_eq!(input_line(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn with_no_hash_nothing_is_run() {
        let missing = Path::new("/definitely/not/chpasswd");
        assert_eq!(
            set_password(missing, None).expect("no run"),
            Outcome::Locked
        );
    }

    #[test]
    fn an_unusable_hash_never_reaches_the_program() {
        let missing = Path::new("/definitely/not/chpasswd");
        let error = set_password(missing, Some("$6$a:b")).expect_err("refused");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(!error.to_string().contains("$6$"), "{error}");
    }

    #[cfg(unix)]
    mod with_a_fake_chpasswd {
        use super::*;
        use std::fs;
        use std::os::unix::fs::PermissionsExt;
        use std::sync::Mutex;

        /// Starting a program while another test still has its script open for writing fails with
        /// "Text file busy" (the child inherits the open file). These tests take turns.
        static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

        /// A stand-in that records its arguments and what it reads on stdin.
        fn fake(dir: &Path, exit_code: i32) -> std::path::PathBuf {
            let path = dir.join("chpasswd");
            let script = format!(
                "#!/bin/sh\necho \"$@\" > '{d}/args'\ncat > '{d}/stdin'\nexit {exit_code}\n",
                d = dir.display()
            );
            fs::write(&path, script).expect("script");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod");
            path
        }

        #[test]
        fn the_hash_goes_on_stdin_and_never_in_the_arguments() {
            let _turn = ONE_AT_A_TIME
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let dir = tempfile::TempDir::new().expect("temp dir");
            let program = fake(dir.path(), 0);

            assert_eq!(
                set_password(&program, Some(HASH)).expect("set"),
                Outcome::Set
            );
            assert_eq!(
                fs::read_to_string(dir.path().join("stdin")).expect("stdin"),
                format!("ckadmin:{HASH}\n")
            );
            let args = fs::read_to_string(dir.path().join("args")).expect("args");
            assert_eq!(args.trim(), "-e");
            assert!(!args.contains(HASH));
        }

        #[test]
        fn a_failing_chpasswd_is_an_error_without_the_hash() {
            let _turn = ONE_AT_A_TIME
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let dir = tempfile::TempDir::new().expect("temp dir");
            let program = fake(dir.path(), 1);

            let error = set_password(&program, Some(HASH)).expect_err("fails");
            assert!(error.to_string().contains("chpasswd failed"), "{error}");
            assert!(!error.to_string().contains(HASH), "{error}");
        }
    }
}
