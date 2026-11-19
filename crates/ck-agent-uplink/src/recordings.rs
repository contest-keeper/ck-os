//! Uploads completed clips separately from heartbeats.

use std::fs::{self, DirEntry, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use ck_protocol::{LocalDateTime, SIGNATURE_HEADER, STICK_HEADER, UploadMode};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use time::{OffsetDateTime, PrimitiveDateTime};
use ureq::http::{Method, Request};

use crate::{MAX_ANSWER_BYTES, Uplink};

const FOLDER: &str = "var/lib/ck/recordings";
const READY: &str = "upload-ready";
const UPLOADED: &str = "uploaded";
const PATH: &str = "/api/v1/recordings";
const CHUNK_LIMIT: u32 = 4 * 1024 * 1024;
const FILE_LIMIT: u64 = 2 * 1024 * 1024 * 1024;
const CHUNKS_PER_PASS: usize = 8;
const FILES_PER_PASS: usize = 8;
const BYTES_PER_SECOND: u64 = 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(12);

#[derive(Debug, thiserror::Error)]
pub enum RecordingError {
    #[error("reading or updating the recordings folder: {0}")]
    Storage(#[from] io::Error),
    #[error("the Hub did not accept the recording request: HTTP {0}")]
    Rejected(u16),
    #[error("the recording request could not be sent to the Hub")]
    Connection,
    #[error("the Hub returned invalid recording progress")]
    Reply,
}

#[derive(Serialize)]
struct Start<'a> {
    file: &'a str,
    kind: &'a str,
    size: u64,
    sha256: &'a str,
}

#[derive(Deserialize)]
struct Progress {
    id: String,
    chunk_size: u32,
    next_chunk: u32,
    complete: bool,
}

#[derive(Deserialize)]
struct NextChunk {
    next_chunk: u32,
}

#[derive(Deserialize)]
struct Complete {
    complete: bool,
}

impl Uplink {
    /// Uploads a bounded batch of closed clips. The caller can run another pass after a failure.
    pub fn upload_recordings(&self) -> Result<usize, RecordingError> {
        let settings = &self.config.settings;
        let folder = self.root.join(FOLDER);
        if let Some(days) = settings.recording.retention_days {
            prune_expired(&folder, days, SystemTime::now())?;
        }
        let Some(url) = self.address(PATH) else {
            return Ok(0);
        };
        if !upload_due(settings.recording.upload, settings.contest.end) {
            return Ok(0);
        }
        let stick = self.config.stick_id.as_str();
        let pending = folder.join(READY).join(stick);
        let uploaded = folder.join(UPLOADED).join(stick);
        let hub = RecordingHub::new(self, url);
        let mut chunks = CHUNKS_PER_PASS;
        let mut sent = 0;
        for path in closed_files(&pending)? {
            if sent == FILES_PER_PASS || chunks == 0 {
                break;
            }
            let mut file = File::open(&path)?;
            let size = file.metadata()?.len();
            if size == 0 || size > FILE_LIMIT {
                continue;
            }
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or(RecordingError::Reply)?;
            let Some(kind) = recording_kind(&path) else {
                continue;
            };
            let sha256 = file_hash(&mut file, size)?;
            let start = Start {
                file: name,
                kind,
                size,
                sha256: &sha256,
            };
            if !hub.upload(&mut file, &start, &mut chunks)? {
                break;
            }
            // Keep the verified local copy, but out of the upload queue.
            drop(file);
            fs::create_dir_all(&uploaded)?;
            let destination = uploaded.join(name);
            if destination.try_exists()? {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "an uploaded recording already uses that name",
                )
                .into());
            }
            fs::rename(path.as_path(), destination)?;
            sent += 1;
        }
        Ok(sent)
    }
}

fn upload_due(mode: UploadMode, end: Option<LocalDateTime>) -> bool {
    match mode {
        UploadMode::DuringContest => true,
        UploadMode::AfterContest => end.is_some_and(|end| {
            OffsetDateTime::now_local().is_ok_and(|now| {
                PrimitiveDateTime::new(now.date(), now.time()) >= end.as_primitive()
            })
        }),
        _ => false,
    }
}

fn entries(directory: &Path) -> io::Result<Vec<DirEntry>> {
    match fs::read_dir(directory) {
        Ok(entries) => entries.collect(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

fn closed_files(directory: &Path) -> io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in entries(directory)? {
        let path = entry.path();
        if entry.file_type()?.is_file() && recording_kind(&path).is_some() {
            files.push(path);
        }
    }
    files.sort_unstable();
    Ok(files)
}

fn recording_kind(path: &Path) -> Option<&'static str> {
    let name = path.file_name()?.to_str()?;
    if name.len() > 128
        || name.starts_with('.')
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return None;
    }
    match path.extension()?.to_str()? {
        "mkv" => Some("video"),
        "jpg" => Some("snapshot"),
        _ => None,
    }
}

fn prune_expired(folder: &Path, days: u32, now: SystemTime) -> io::Result<()> {
    let mut directories = vec![folder.to_path_buf(), folder.join("snapshots")];
    for queue in [READY, UPLOADED] {
        for entry in entries(&folder.join(queue))? {
            if entry.file_type()?.is_dir() {
                directories.push(entry.path());
            }
        }
    }
    let maximum_age = Duration::from_secs(u64::from(days) * 86_400);
    for directory in directories {
        for path in closed_files(&directory)? {
            let modified = fs::metadata(&path)?.modified()?;
            if now
                .duration_since(modified)
                .is_ok_and(|age| age >= maximum_age)
            {
                fs::remove_file(path)?;
            }
        }
    }
    Ok(())
}

fn file_hash(file: &mut File, size: u64) -> io::Result<String> {
    let mut hash = Sha256::new();
    let mut bytes = file.take(size + 1);
    let mut buffer = [0; 16 * 1024];
    let mut read = 0;
    loop {
        let count = bytes.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
        read += count as u64;
    }
    if read != size {
        return Err(io::Error::other("the closed recording changed size"));
    }
    Ok(hex::encode(hash.finalize()))
}

struct RecordingHub<'a> {
    stick: &'a Uplink,
    base: String,
    agent: ureq::Agent,
}

impl<'a> RecordingHub<'a> {
    fn new(stick: &'a Uplink, base: String) -> Self {
        let agent = ureq::Agent::config_builder()
            .proxy(None)
            .max_redirects(0)
            .timeout_global(Some(TIMEOUT))
            .build()
            .into();
        Self { stick, base, agent }
    }

    fn upload(
        &self,
        file: &mut File,
        start: &Start<'_>,
        remaining: &mut usize,
    ) -> Result<bool, RecordingError> {
        let body = serde_json::to_vec(start).map_err(|_| RecordingError::Reply)?;
        let begun: Progress = self.call(Method::POST, "", &body, &body, None)?;
        let total = validate(&begun, start.size)?;
        if begun.complete {
            return Ok(true);
        }
        let suffix = format!("/{}", begun.id);
        let mut next = begun.next_chunk;
        if next > 0 {
            let signed = format!("ck-recording-status\n{}", begun.id);
            let status: Progress = self.call(Method::GET, &suffix, &[], signed.as_bytes(), None)?;
            validate(&status, start.size)?;
            if status.id != begun.id || status.chunk_size != begun.chunk_size {
                return Err(RecordingError::Reply);
            }
            if status.complete {
                return Ok(true);
            }
            next = status.next_chunk;
        }
        let chunk_size = u64::from(begun.chunk_size);
        let mut buffer = vec![0; usize::try_from(chunk_size).map_err(|_| RecordingError::Reply)?];
        while next < total && *remaining > 0 {
            let offset = u64::from(next) * chunk_size;
            let length = (start.size - offset).min(chunk_size);
            let bytes =
                &mut buffer[..usize::try_from(length).map_err(|_| RecordingError::Reply)?];
            file.seek(SeekFrom::Start(offset))?;
            file.read_exact(bytes)?;
            let digest = ck_crypto::sha256_hex(bytes);
            let signed = format!("ck-recording-chunk\n{}\n{next}\n{digest}", begun.id);
            let started = Instant::now();
            let reply: NextChunk = self.call(
                Method::PUT,
                &format!("{suffix}/chunks/{next}"),
                bytes,
                signed.as_bytes(),
                Some(&digest),
            )?;
            if reply.next_chunk <= next || reply.next_chunk > total {
                return Err(RecordingError::Reply);
            }
            *remaining -= 1;
            next = reply.next_chunk;
            let interval = Duration::from_nanos(length * 1_000_000_000 / BYTES_PER_SECOND);
            std::thread::sleep(interval.saturating_sub(started.elapsed()));
        }
        if next < total {
            return Ok(false);
        }
        let signed = format!("ck-recording-complete\n{}", begun.id);
        let reply: Complete = self.call(
            Method::POST,
            &format!("{suffix}/complete"),
            &[],
            signed.as_bytes(),
            None,
        )?;
        if !reply.complete {
            return Err(RecordingError::Reply);
        }
        Ok(true)
    }

    fn call<T: DeserializeOwned>(
        &self,
        method: Method,
        suffix: &str,
        body: &[u8],
        signed: &[u8],
        chunk: Option<&str>,
    ) -> Result<T, RecordingError> {
        let signature = ck_crypto::sign(&self.stick.config.token, signed);
        let mut builder = Request::builder()
            .method(method)
            .uri(format!("{}{suffix}", self.base))
            .header(STICK_HEADER, self.stick.config.stick_id.as_str())
            .header(SIGNATURE_HEADER, signature);
        if let Some(chunk) = chunk {
            builder = builder
                .header("x-ck-chunk-sha256", chunk)
                .header("content-type", "application/octet-stream");
        } else if !body.is_empty() {
            builder = builder.header("content-type", "application/json");
        }
        let request = builder.body(body).map_err(|_| RecordingError::Connection)?;
        let mut response = self.agent.run(request).map_err(|error| match error {
            ureq::Error::StatusCode(status) => RecordingError::Rejected(status),
            _ => RecordingError::Connection,
        })?;
        if !response.status().is_success() {
            return Err(RecordingError::Rejected(response.status().as_u16()));
        }
        let answer = response
            .body_mut()
            .with_config()
            .limit(MAX_ANSWER_BYTES)
            .read_to_vec()
            .map_err(|_| RecordingError::Reply)?;
        serde_json::from_slice(&answer).map_err(|_| RecordingError::Reply)
    }
}

fn validate(status: &Progress, size: u64) -> Result<u32, RecordingError> {
    if status.id.len() != 32
        || !status
            .id
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        || !(1..=CHUNK_LIMIT).contains(&status.chunk_size)
    {
        return Err(RecordingError::Reply);
    }
    let total = u32::try_from(size.div_ceil(u64::from(status.chunk_size)))
        .map_err(|_| RecordingError::Reply)?;
    if status.next_chunk > total || (status.complete && status.next_chunk != total) {
        return Err(RecordingError::Reply);
    }
    Ok(total)
}
