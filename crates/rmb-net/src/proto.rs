// SPDX-License-Identifier: LGPL-2.1-or-later
//! Messages between the render server and its clients: JSON with a length in front, followed
//! by raw bytes for the messages that carry a file or an image.

use std::io::{Read, Write};

use serde::{Deserialize, Serialize};

/// Hardware of a client (no user data): shown to the user and used for the first guess of a
/// client's speed.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Specs {
    pub cpu: String,
    pub cores: usize,
    pub threads: usize,
    pub ram_gb: f64,
    /// Graphics card, or empty when the client renders on the CPU.
    pub gpu: String,
    pub os: String,
}

impl Specs {
    /// The hardware of this machine; `gpu` is the graphics card the client renders with.
    pub fn of_this_machine(gpu: &str) -> Specs {
        use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};
        let sys = System::new_with_specifics(RefreshKind::nothing().with_cpu(CpuRefreshKind::nothing()).with_memory(MemoryRefreshKind::nothing().with_ram())).ok();
        Specs {
            cpu: sys.as_ref().and_then(|s| s.cpus().first().map(|c| c.brand().trim().to_string())).unwrap_or_default(),
            cores: System::physical_core_count().unwrap_or(0),
            threads: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1),
            ram_gb: (sys.as_ref().map_or(0, |s| s.total_memory()) as f64 / (1u64 << 30) as f64 * 10.0).round() / 10.0,
            gpu: gpu.to_string(),
            os: std::env::consts::OS.to_string(),
        }
    }

    /// One line for lists: "RTX A2000, 8 cores, 32 GB".
    pub fn summary(&self) -> String {
        let device = if self.gpu.is_empty() { format!("CPU {}", self.cpu) } else { self.gpu.clone() };
        format!("{device}, {} cores, {:.0} GB, {}", self.cores.max(1), self.ram_gb, self.os)
    }
}

/// A piece of work: one frame of an animation, or a range of samples of a still image.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "k")]
pub enum Unit {
    Frame { f: usize },
    Samples { start: u32, count: u32 },
}

impl Unit {
    pub fn label(&self) -> String {
        match self {
            Unit::Frame { f } => format!("frame {f}"),
            Unit::Samples { start, count } => format!("samples {}-{}", start, start + count - 1),
        }
    }

    /// Relative amount of work (speed estimates).
    pub fn weight(&self, samples_per_frame: u32) -> f64 {
        match self {
            Unit::Frame { .. } => samples_per_frame.max(1) as f64,
            Unit::Samples { count, .. } => *count as f64,
        }
    }
}

/// A file the scene needs, as the server knows it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileEntry {
    /// The server's absolute path (the client recreates it below its cache).
    pub path: String,
    pub size: u64,
    /// SHA-256, hex.
    pub hash: String,
    /// A Mandelbulb3D map of the formulas (goes into the client's map folder by its name).
    pub map: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "k")]
pub enum JobKind {
    /// Frames `from..=to` of the scene's animation.
    Animation { from: usize, to: usize },
    /// One image; the clients render ranges of its samples.
    Still,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobSpec {
    pub job_id: u64,
    /// The scene as JSON (file paths are the server's, see `FileEntry`).
    pub scene: String,
    pub width: u32,
    pub height: u32,
    pub samples: u32,
    pub kind: JobKind,
    pub files: Vec<FileEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "t")]
pub enum Msg {
    /// Client, first message of a connection.
    Welcome { version: String, name: String, id: u64, nonce: String, key: bool, busy: bool, specs: Specs },
    /// Server: `auth` = SHA-256 of nonce and key (when the client needs a key).
    Hello { version: String, server: String, auth: Option<String> },
    Accepted,
    Rejected { reason: String },
    Job(Box<JobSpec>),
    /// Client: files it does not have yet.
    Need { hashes: Vec<String> },
    /// Server: one file (bytes follow).
    File { hash: String, bytes: u64 },
    FilesDone,
    /// Client: ready for the next piece of work.
    Ready,
    Work { unit: Unit, done: usize, total: usize },
    /// Server: nothing to do right now, ask again shortly.
    Wait,
    /// Server: the job is finished.
    Done,
    Progress { unit: Unit, fraction: f32 },
    Continue { done: usize, total: usize },
    /// Server: stop the current piece (another client finished it first).
    Cancel,
    /// Client: a finished piece (bytes follow: PNG for frames, float data for samples).
    Result { unit: Unit, hash: String, bytes: u64 },
    /// Server: stored (`ok`), so the client may delete its copy; not ok: send again.
    Ack { unit: Unit, ok: bool },
    Failed { unit: Unit, error: String },
    Bye,
}

impl Msg {
    /// Length of the raw bytes after this message.
    fn blob_len(&self) -> u64 {
        match self {
            Msg::File { bytes, .. } | Msg::Result { bytes, .. } => *bytes,
            _ => 0,
        }
    }
}

/// Largest JSON message (scenes with their formulas are a few MB at most).
const MAX_JSON: usize = 256 << 20;

/// Sends `m` and, for file and result messages, `blob` (its length must match the message).
pub fn send(w: &mut impl Write, m: &Msg, blob: Option<&[u8]>) -> std::io::Result<()> {
    let json = serde_json::to_vec(m).map_err(std::io::Error::other)?;
    w.write_all(&(json.len() as u32).to_le_bytes())?;
    w.write_all(&json)?;
    if let Some(b) = blob {
        w.write_all(b)?;
    }
    w.flush()
}

/// Receives a message and its raw bytes (empty for messages without).
pub fn recv(r: &mut impl Read) -> std::io::Result<(Msg, Vec<u8>)> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_JSON {
        return Err(std::io::Error::other("message too large"));
    }
    let mut json = vec![0u8; len];
    r.read_exact(&mut json)?;
    let m: Msg = serde_json::from_slice(&json).map_err(std::io::Error::other)?;
    let n = m.blob_len();
    let mut blob = Vec::new();
    if n > 0 {
        blob.try_reserve_exact(n as usize).map_err(std::io::Error::other)?;
        r.take(n).read_to_end(&mut blob)?;
        if blob.len() as u64 != n {
            return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "connection closed during a transfer"));
        }
    }
    Ok((m, blob))
}

/// SHA-256 of `data` as hex.
pub fn sha256(data: &[u8]) -> String {
    use sha2::Digest;
    hex(&sha2::Sha256::digest(data))
}

/// The key proof: SHA-256 of the nonce and the key.
pub fn auth(nonce: &str, key: &str) -> String {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    h.update(nonce.as_bytes());
    h.update([0]);
    h.update(key.as_bytes());
    hex(&h.finalize())
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|v| format!("{v:02x}")).collect()
}

/// A random number (seeded from the process' random hasher keys and the time).
pub fn random_u64() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
    h.write_u32(std::process::id());
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_and_blobs_round_trip() {
        let mut buf = Vec::new();
        send(&mut buf, &Msg::Ready, None).unwrap();
        let data = vec![7u8; 1000];
        send(&mut buf, &Msg::Result { unit: Unit::Frame { f: 3 }, hash: sha256(&data), bytes: data.len() as u64 }, Some(&data)).unwrap();
        let mut r = std::io::Cursor::new(buf);
        assert!(matches!(recv(&mut r).unwrap().0, Msg::Ready));
        let (m, blob) = recv(&mut r).unwrap();
        let Msg::Result { unit, hash, .. } = m else { panic!("{m:?}") };
        assert_eq!(unit, Unit::Frame { f: 3 });
        assert_eq!(hash, sha256(&blob));
        assert_eq!(blob, data);
        assert_ne!(auth("n", "a"), auth("n", "b"));
    }
}
