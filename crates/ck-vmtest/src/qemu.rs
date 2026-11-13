//! Starts QEMU for one test boot and talks to it over QMP, QEMU's JSON control protocol.

use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use serde_json::{Value, json};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },
    #[error("QEMU did not answer on its control port: {0}")]
    Qmp(String),
}

fn io(context: impl Into<String>) -> impl FnOnce(std::io::Error) -> Error {
    let context = context.into();
    move |source| Error::Io { context, source }
}

pub struct Options {
    pub qemu: PathBuf,
    pub image: PathBuf,
    pub firmware: PathBuf,
    pub vars_template: PathBuf,
    pub work: PathBuf,
    pub memory_mb: u32,
    pub accel: String,
}

/// One running test machine. It is stopped when this value is dropped.
pub struct Machine {
    child: Child,
    qmp_port: u16,
    serial_log: PathBuf,
}

impl Machine {
    /// Boots the image headless. The image is opened with `snapshot=on`, so the test never changes
    /// it, and the machine is told it is a test machine so the agent writes its report.
    pub fn start(options: &Options) -> Result<Self, Error> {
        // Fresh firmware variables every time, so an earlier boot cannot change this one.
        let vars = options.work.join("OVMF_VARS.fd");
        fs::copy(&options.vars_template, &vars).map_err(io("copying the firmware variables"))?;
        let code = options.work.join("OVMF_CODE.fd");
        fs::copy(&options.firmware, &code).map_err(io("copying the firmware"))?;
        let serial_log = options.work.join("serial.log");
        let qmp_port = free_port()?;

        let child = Command::new(&options.qemu)
            .args([
                "-machine",
                "q35",
                "-accel",
                &options.accel,
                "-cpu",
                "qemu64",
            ])
            .args(["-smp", "2", "-m", &options.memory_mb.to_string()])
            .arg("-drive")
            .arg(format!(
                "if=pflash,format=raw,readonly=on,file={}",
                code.display()
            ))
            .arg("-drive")
            .arg(format!("if=pflash,format=raw,file={}", vars.display()))
            .arg("-drive")
            .arg(format!(
                "file={},format=raw,if=none,id=d0,snapshot=on,cache=unsafe,aio=threads",
                options.image.display()
            ))
            .args(["-device", "virtio-blk-pci,drive=d0", "-device", "qemu-xhci"])
            .args([
                "-device",
                "usb-tablet",
                "-device",
                "usb-kbd",
                "-device",
                "VGA,vgamem_mb=64",
            ])
            .args([
                "-netdev",
                "user,id=n0",
                "-device",
                "virtio-net-pci,netdev=n0",
            ])
            .args(["-smbios", "type=11,value=ck-selftest=1"])
            .args(["-display", "none"])
            .arg("-qmp")
            .arg(format!("tcp:127.0.0.1:{qmp_port},server,nowait"))
            .arg("-serial")
            .arg(format!("file:{}", serial_log.display()))
            .stdout(Stdio::null())
            .stderr(File::create(options.work.join("qemu.err")).map_err(io("creating qemu.err"))?)
            .spawn()
            .map_err(io(format!("starting {}", options.qemu.display())))?;
        Ok(Self {
            child,
            qmp_port,
            serial_log,
        })
    }

    /// Whether QEMU has stopped by itself, for example because its options were wrong.
    pub fn exited(&mut self) -> Option<String> {
        match self.child.try_wait() {
            Ok(Some(status)) => Some(status.to_string()),
            Ok(None) => None,
            Err(error) => Some(error.to_string()),
        }
    }

    pub fn serial_text(&self) -> String {
        fs::read(&self.serial_log)
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_default()
    }

    pub fn screenshot(&self, png_path: &Path) -> Result<(), Error> {
        let ppm_path = png_path.with_extension("ppm");
        let mut qmp = Qmp::connect(self.qmp_port)?;
        qmp.command(
            "screendump",
            &json!({ "filename": ppm_path.to_string_lossy() }),
        )?;
        let ppm = fs::read(&ppm_path).map_err(io("reading the screen dump"))?;
        write_png(&ppm, png_path)?;
        let _ = fs::remove_file(&ppm_path);
        Ok(())
    }
}

impl Drop for Machine {
    fn drop(&mut self) {
        // Ask politely, then make sure.
        if let Ok(mut qmp) = Qmp::connect(self.qmp_port) {
            let _ = qmp.command("quit", &json!({}));
        }
        std::thread::sleep(Duration::from_millis(500));
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn free_port() -> Result<u16, Error> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(io("finding a free port"))?;
    Ok(listener
        .local_addr()
        .map_err(io("reading the port"))?
        .port())
}

struct Qmp {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
}

impl Qmp {
    fn connect(port: u16) -> Result<Self, Error> {
        let stream = TcpStream::connect(("127.0.0.1", port)).map_err(io("connecting to QMP"))?;
        stream
            .set_read_timeout(Some(Duration::from_secs(30)))
            .map_err(io("setting the QMP timeout"))?;
        let writer = stream
            .try_clone()
            .map_err(io("copying the QMP connection"))?;
        let mut qmp = Self {
            reader: BufReader::new(stream),
            writer,
        };
        qmp.read_message()?; // the greeting
        qmp.command("qmp_capabilities", &json!({}))?;
        Ok(qmp)
    }

    fn read_message(&mut self) -> Result<Value, Error> {
        let mut line = String::new();
        self.reader
            .read_line(&mut line)
            .map_err(io("reading from QMP"))?;
        serde_json::from_str(&line).map_err(|e| Error::Qmp(format!("{e}: {line}")))
    }

    fn command(&mut self, name: &str, arguments: &Value) -> Result<Value, Error> {
        let request = json!({ "execute": name, "arguments": arguments });
        writeln!(self.writer, "{request}").map_err(io("writing to QMP"))?;
        loop {
            let message = self.read_message()?;
            if message.get("return").is_some() {
                return Ok(message);
            }
            if let Some(error) = message.get("error") {
                return Err(Error::Qmp(error.to_string()));
            }
            // Anything else is an event, such as a shutdown notice: keep waiting for the answer.
        }
    }
}

/// QEMU's screen dump is a binary PPM (`P6`); PNG opens everywhere.
fn write_png(ppm: &[u8], path: &Path) -> Result<(), Error> {
    let bad = || Error::Qmp("the screen dump is not a P6 image".to_owned());
    let mut parts = ppm.splitn(4, |byte| *byte == b'\n');
    let (magic, size, _max, pixels) = (
        parts.next().ok_or_else(bad)?,
        parts.next().ok_or_else(bad)?,
        parts.next().ok_or_else(bad)?,
        parts.next().ok_or_else(bad)?,
    );
    if magic != b"P6" {
        return Err(bad());
    }
    let size = String::from_utf8_lossy(size);
    let mut numbers = size.split_whitespace().map(str::parse::<u32>);
    let (Some(Ok(width)), Some(Ok(height))) = (numbers.next(), numbers.next()) else {
        return Err(bad());
    };
    let file = File::create(path).map_err(io("creating the screenshot"))?;
    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|e| Error::Qmp(e.to_string()))?;
    writer
        .write_image_data(pixels)
        .map_err(|e| Error::Qmp(e.to_string()))
}
