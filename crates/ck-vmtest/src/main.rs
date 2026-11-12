//! `ck-vmtest`: boots a stick image in QEMU and says PASS or FAIL.
//!
//! The image must already hold the stick's settings (made by `ck-forge image build`). The machine
//! boots with the image opened read-only (`snapshot=on`), the agent writes a report to the serial
//! port, and the report is compared with the settings stored in the image itself.

mod check;
mod qemu;

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use ck_protocol::StickConfig;
use clap::Parser;

use qemu::{Machine, Options};

const EXIT_FAIL: u8 = 1;
/// The test itself could not run (no QEMU, unreadable image), so nothing is known about the image.
const EXIT_CANNOT_RUN: u8 = 2;

#[derive(Parser)]
#[command(
    name = "ck-vmtest",
    version,
    about = "Boot a stick image in QEMU and check the settings were applied"
)]
struct Cli {
    /// The stick image to test. It is never changed
    #[arg(long)]
    image: PathBuf,
    /// Where to save the report, the serial log and, on failure, a screenshot
    #[arg(long, default_value = "vmtest-result")]
    out: PathBuf,
    /// The QEMU program
    #[arg(long, default_value = r"C:\Program Files\qemu\qemu-system-x86_64.exe")]
    qemu: PathBuf,
    /// UEFI firmware (default: the one that comes with QEMU)
    #[arg(long)]
    firmware: Option<PathBuf>,
    /// UEFI variable store to start from (default: the one that comes with QEMU)
    #[arg(long)]
    vars: Option<PathBuf>,
    /// QEMU acceleration: whpx on Windows, tcg works everywhere but is slow
    #[arg(long, default_value = "whpx")]
    accel: String,
    /// Memory for the machine, in MB
    #[arg(long, default_value_t = 3072)]
    memory: u32,
    /// Seconds to wait for the report
    #[arg(long, default_value_t = 240)]
    timeout: u64,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let mut out = io::stdout();
    match run(&cli, &mut out) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(EXIT_FAIL),
        Err(message) => {
            eprintln!("cannot run the test: {message}");
            ExitCode::from(EXIT_CANNOT_RUN)
        }
    }
}

fn run(cli: &Cli, out: &mut impl Write) -> Result<bool, String> {
    if !cli.qemu.exists() {
        return Err(format!(
            "QEMU is not at {}: pass --qemu with the path of qemu-system-x86_64",
            cli.qemu.display()
        ));
    }
    let config = read_config(&cli.image)?;
    fs::create_dir_all(&cli.out).map_err(|e| format!("creating {}: {e}", cli.out.display()))?;
    let work = tempfile::TempDir::new().map_err(|e| format!("creating a work folder: {e}"))?;

    let share = cli.qemu.parent().map(|dir| dir.join("share"));
    let options = Options {
        qemu: cli.qemu.clone(),
        image: cli.image.clone(),
        firmware: cli
            .firmware
            .clone()
            .or_else(|| share.as_ref().map(|s| s.join("edk2-x86_64-code.fd")))
            .ok_or("no firmware: pass --firmware")?,
        vars_template: cli
            .vars
            .clone()
            .or_else(|| share.as_ref().map(|s| s.join("edk2-i386-vars.fd")))
            .ok_or("no firmware variables: pass --vars")?,
        work: work.path().to_path_buf(),
        memory_mb: cli.memory,
        accel: cli.accel.clone(),
    };

    say(out, format_args!("Booting {} ...", cli.image.display()))?;
    let started = Instant::now();
    let mut machine = Machine::start(&options).map_err(|e| e.to_string())?;
    let outcome = wait_for_report(&mut machine, Duration::from_secs(cli.timeout));
    let waited = started.elapsed().as_secs();
    let log = machine.serial_text();
    let _ = fs::write(cli.out.join("serial.log"), &log);

    let problems = match outcome {
        Outcome::Report(report) => {
            let json = serde_json::to_string_pretty(&report).unwrap_or_default();
            let _ = fs::write(cli.out.join("report.json"), json);
            say(
                out,
                format_args!(
                    "Report received after {waited} s (the machine had been up {:.0} s)",
                    report.uptime_seconds
                ),
            )?;
            check::problems(&config, &report)
        }
        Outcome::Unreadable(error) => vec![format!("the report cannot be read: {error}")],
        Outcome::MachineStopped(status) => vec![format!("QEMU stopped by itself ({status})")],
        Outcome::TimedOut => vec![format!("no report within {} s", cli.timeout)],
    };

    if problems.is_empty() {
        say(
            out,
            format_args!("PASS: the machine matches the settings in the image"),
        )?;
        return Ok(true);
    }
    say(out, format_args!("FAIL:"))?;
    for problem in &problems {
        say(out, format_args!("  - {problem}"))?;
    }
    save_screenshot(&machine, &cli.out, out)?;
    Ok(false)
}

enum Outcome {
    Report(ck_selftest::Report),
    Unreadable(String),
    MachineStopped(String),
    TimedOut,
}

fn wait_for_report(machine: &mut Machine, timeout: Duration) -> Outcome {
    let deadline = Instant::now() + timeout;
    loop {
        match ck_selftest::find_report(&machine.serial_text()) {
            Some(Ok(report)) => return Outcome::Report(report),
            Some(Err(error)) => return Outcome::Unreadable(error.to_string()),
            None => {}
        }
        if let Some(status) = machine.exited() {
            return Outcome::MachineStopped(status);
        }
        if Instant::now() >= deadline {
            return Outcome::TimedOut;
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}

fn read_config(image: &Path) -> Result<StickConfig, String> {
    let bytes = ck_image::read_file(image, "ck/config.json")
        .map_err(|e| format!("reading the settings from {}: {e}", image.display()))?;
    serde_json::from_slice(&bytes)
        .map_err(|e| format!("the settings in the image are not valid: {e}"))
}

/// A screenshot only helps when the machine is still running, so a missing one is not an error.
fn save_screenshot(machine: &Machine, out_dir: &Path, out: &mut impl Write) -> Result<(), String> {
    let path = out_dir.join("screenshot.png");
    match machine.screenshot(&path) {
        Ok(()) => say(out, format_args!("Screenshot: {}", path.display())),
        Err(error) => say(out, format_args!("No screenshot: {error}")),
    }
}

fn say(out: &mut impl Write, text: std::fmt::Arguments) -> Result<(), String> {
    writeln!(out, "{text}").map_err(|e| format!("writing the output: {e}"))
}
