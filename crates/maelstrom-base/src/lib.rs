//! Core structs used by the broker, worker, and clients. Everything in this crate must be usable
//! from wasm.

pub mod manifest;
pub mod proto;
pub mod ring_buffer;
pub mod stats;
pub mod tty;

pub use camino::{Utf8Component, Utf8Path, Utf8PathBuf};
pub use enumset::{enum_set, EnumSet};
pub use nonempty::{nonempty, NonEmpty};

use derive_more::{Constructor, Display, From};
use enumset::EnumSetType;
use hex::{self, FromHexError};
use serde::{Deserialize, Serialize};
use std::{
    error::Error,
    fmt::{self, Debug, Formatter},
    hash::Hash,
    num::NonZeroU32,
    result::Result,
    str::{self, FromStr},
    time::Duration,
};
use strum::EnumIter;

/// ID of a client connection. These share the same ID space as [`WorkerId`].
#[derive(
    Copy, Clone, Debug, Deserialize, Display, Eq, From, Hash, Ord, PartialEq, PartialOrd, Serialize,
)]
pub struct ClientId(u32);

impl ClientId {
    pub fn as_u32(&self) -> u32 {
        self.0
    }
}

/// A client-relative job ID. Clients can assign these however they like.
#[derive(
    Copy, Clone, Debug, Deserialize, Display, Eq, From, Hash, Ord, PartialEq, PartialOrd, Serialize,
)]
pub struct ClientJobId(u32);

impl ClientJobId {
    pub fn from_u32(v: u32) -> Self {
        Self(v)
    }

    pub fn as_u32(&self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum ArtifactType {
    /// A .tar file
    Tar,
    /// A serialized `Manifest`
    Manifest,
}

/// An absolute job ID that includes a [`ClientId`] for disambiguation.
#[derive(Copy, Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct JobId {
    pub cid: ClientId,
    pub cjid: ClientJobId,
}

#[derive(Debug, Deserialize, EnumIter, EnumSetType, Serialize)]
pub enum JobDevice {
    Full,
    Fuse,
    Null,
    Random,
    Shm,
    Tty,
    Urandom,
    Zero,
}

#[derive(Debug, Deserialize, EnumSetType, Serialize)]
#[serde(rename_all = "kebab-case")]
#[enumset(serialize_repr = "list")]
pub enum JobDeviceForTomlAndJson {
    Full,
    Fuse,
    Null,
    Random,
    Shm,
    Tty,
    Urandom,
    Zero,
}

impl From<JobDeviceForTomlAndJson> for JobDevice {
    fn from(value: JobDeviceForTomlAndJson) -> JobDevice {
        match value {
            JobDeviceForTomlAndJson::Full => JobDevice::Full,
            JobDeviceForTomlAndJson::Fuse => JobDevice::Fuse,
            JobDeviceForTomlAndJson::Null => JobDevice::Null,
            JobDeviceForTomlAndJson::Random => JobDevice::Random,
            JobDeviceForTomlAndJson::Shm => JobDevice::Shm,
            JobDeviceForTomlAndJson::Tty => JobDevice::Tty,
            JobDeviceForTomlAndJson::Urandom => JobDevice::Urandom,
            JobDeviceForTomlAndJson::Zero => JobDevice::Zero,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type")]
#[serde(rename_all = "kebab-case")]
#[serde(deny_unknown_fields)]
pub enum JobMountForTomlAndJson {
    Bind {
        mount_point: Utf8PathBuf,
        local_path: Utf8PathBuf,
        #[serde(default)]
        read_only: bool,
    },
    Devices {
        devices: EnumSet<JobDeviceForTomlAndJson>,
    },
    Devpts {
        mount_point: Utf8PathBuf,
    },
    Mqueue {
        mount_point: Utf8PathBuf,
    },
    Proc {
        mount_point: Utf8PathBuf,
    },
    Sys {
        mount_point: Utf8PathBuf,
    },
    Tmp {
        mount_point: Utf8PathBuf,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub enum JobMount {
    Bind {
        mount_point: Utf8PathBuf,
        local_path: Utf8PathBuf,
        read_only: bool,
    },
    Devices {
        devices: EnumSet<JobDevice>,
    },
    Devpts {
        mount_point: Utf8PathBuf,
    },
    Mqueue {
        mount_point: Utf8PathBuf,
    },
    Proc {
        mount_point: Utf8PathBuf,
    },
    Sys {
        mount_point: Utf8PathBuf,
    },
    Tmp {
        mount_point: Utf8PathBuf,
    },
}

impl From<JobMountForTomlAndJson> for JobMount {
    fn from(job_mount: JobMountForTomlAndJson) -> JobMount {
        match job_mount {
            JobMountForTomlAndJson::Bind {
                mount_point,
                local_path,
                read_only,
            } => JobMount::Bind {
                mount_point,
                local_path,
                read_only,
            },
            JobMountForTomlAndJson::Devices { devices } => JobMount::Devices {
                devices: devices.into_iter().map(JobDevice::from).collect(),
            },
            JobMountForTomlAndJson::Devpts { mount_point } => JobMount::Devpts { mount_point },
            JobMountForTomlAndJson::Mqueue { mount_point } => JobMount::Mqueue { mount_point },
            JobMountForTomlAndJson::Proc { mount_point } => JobMount::Proc { mount_point },
            JobMountForTomlAndJson::Sys { mount_point } => JobMount::Sys { mount_point },
            JobMountForTomlAndJson::Tmp { mount_point } => JobMount::Tmp { mount_point },
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum JobNetwork {
    #[default]
    Disabled,
    Loopback,
    Local,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum JobRootOverlay {
    #[default]
    None,
    Tmp,
    Local {
        upper: Utf8PathBuf,
        work: Utf8PathBuf,
    },
}

/// ID of a user. This should be compatible with uid_t.
#[derive(
    Copy, Clone, Debug, Deserialize, Display, Eq, From, Hash, Ord, PartialEq, PartialOrd, Serialize,
)]
pub struct UserId(u32);

impl UserId {
    pub fn new(v: u32) -> Self {
        Self(v)
    }

    pub fn as_u32(&self) -> u32 {
        self.0
    }
}

/// ID of a group. This should be compatible with gid_t.
#[derive(
    Copy, Clone, Debug, Deserialize, Display, Eq, From, Hash, Ord, PartialEq, PartialOrd, Serialize,
)]
pub struct GroupId(u32);

impl GroupId {
    pub fn new(v: u32) -> Self {
        Self(v)
    }

    pub fn as_u32(&self) -> u32 {
        self.0
    }
}

/// A count of seconds.
#[derive(Copy, Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct Timeout(NonZeroU32);

impl Timeout {
    pub fn new(timeout: u32) -> Option<Self> {
        NonZeroU32::new(timeout).map(Timeout)
    }

    pub fn as_u32(&self) -> u32 {
        self.0.into()
    }
}

impl From<Timeout> for Duration {
    fn from(timeout: Timeout) -> Duration {
        Duration::from_secs(timeout.0.get().into())
    }
}

/// The size of a terminal in characters.
#[derive(Copy, Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct WindowSize {
    pub rows: u16,
    pub columns: u16,
}

impl WindowSize {
    pub fn new(rows: u16, columns: u16) -> Self {
        Self { rows, columns }
    }
}

/// The parameters for a TTY for a job.
#[derive(Copy, Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct JobTty {
    /// A Unix domain socket abstract address. We use exactly 6 bytes because that's how many bytes
    /// the autobind feature in Linux uses. The first byte will always be 0.
    pub socket_address: [u8; 6],

    /// The initial window size of the TTY. Window size updates may follow.
    pub window_size: WindowSize,
}

impl JobTty {
    pub fn new(socket_address: &[u8; 6], window_size: WindowSize) -> Self {
        let socket_address = *socket_address;
        Self {
            socket_address,
            window_size,
        }
    }
}

/// All necessary information for the worker to execute a job.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct JobSpec {
    pub program: Utf8PathBuf,
    pub arguments: Vec<String>,
    pub environment: Vec<String>,
    pub layers: NonEmpty<(Sha256Digest, ArtifactType)>,
    pub mounts: Vec<JobMount>,
    pub network: JobNetwork,
    pub root_overlay: JobRootOverlay,
    pub working_directory: Option<Utf8PathBuf>,
    pub user: Option<UserId>,
    pub group: Option<GroupId>,
    pub timeout: Option<Timeout>,
    pub estimated_duration: Option<Duration>,
    pub allocate_tty: Option<JobTty>,
}

impl JobSpec {
    pub fn new(
        program: impl Into<String>,
        layers: impl Into<NonEmpty<(Sha256Digest, ArtifactType)>>,
    ) -> Self {
        JobSpec {
            program: program.into().into(),
            layers: layers.into(),
            arguments: Default::default(),
            environment: Default::default(),
            mounts: Default::default(),
            network: Default::default(),
            root_overlay: Default::default(),
            working_directory: None,
            user: None,
            group: None,
            timeout: None,
            estimated_duration: None,
            allocate_tty: None,
        }
    }

    pub fn arguments<I, T>(mut self, arguments: I) -> Self
    where
        I: IntoIterator<Item = T>,
        T: Into<String>,
    {
        self.arguments = arguments.into_iter().map(Into::into).collect();
        self
    }

    pub fn environment<I, T>(mut self, environment: I) -> Self
    where
        I: IntoIterator<Item = T>,
        T: Into<String>,
    {
        self.environment = environment.into_iter().map(Into::into).collect();
        self
    }

    pub fn mounts(mut self, mounts: impl IntoIterator<Item = JobMount>) -> Self {
        self.mounts = mounts.into_iter().collect();
        self
    }

    pub fn network(mut self, network: JobNetwork) -> Self {
        self.network = network;
        self
    }

    pub fn root_overlay(mut self, root_overlay: JobRootOverlay) -> Self {
        self.root_overlay = root_overlay;
        self
    }

    pub fn working_directory(mut self, working_directory: Option<impl Into<Utf8PathBuf>>) -> Self {
        self.working_directory = working_directory.map(Into::into);
        self
    }

    pub fn user(mut self, user: Option<impl Into<UserId>>) -> Self {
        self.user = user.map(Into::into);
        self
    }

    pub fn group(mut self, group: Option<impl Into<GroupId>>) -> Self {
        self.group = group.map(Into::into);
        self
    }

    pub fn timeout(mut self, timeout: Option<impl Into<Timeout>>) -> Self {
        self.timeout = timeout.map(Into::into);
        self
    }

    pub fn estimated_duration(mut self, estimated_duration: Option<impl Into<Duration>>) -> Self {
        self.estimated_duration = estimated_duration.map(Into::into);
        self
    }

    pub fn allocate_tty(mut self, allocate_tty: Option<impl Into<JobTty>>) -> Self {
        self.allocate_tty = allocate_tty.map(Into::into);
        self
    }

    pub fn must_be_run_locally(&self) -> bool {
        self.network == JobNetwork::Local
            || self
                .mounts
                .iter()
                .any(|mount| matches!(mount, JobMount::Bind { .. }))
            || matches!(&self.root_overlay, JobRootOverlay::Local { .. })
            || self.allocate_tty.is_some()
    }
}

/// How a job's process terminated. A process can either exit of its own accord or be killed by a
/// signal.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum JobStatus {
    Exited(u8),
    Signaled(u8),
}

/// The result for stdout or stderr for a job.
#[derive(Clone, Deserialize, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum JobOutputResult {
    /// There was no output.
    None,

    /// The output is contained in the provided slice.
    Inline(Box<[u8]>),

    /// The output was truncated to the provided slice, the size of which is based on the job
    /// request. The actual size of the output is also provided, though the remaining bytes will
    /// have been thrown away.
    Truncated { first: Box<[u8]>, truncated: u64 },
    /*
     * To come:
    /// The output was stored in a digest, and is of the provided size.
    External(Sha256Digest, u64),
    */
}

impl Debug for JobOutputResult {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            JobOutputResult::None => f.debug_tuple("None").finish(),
            JobOutputResult::Inline(bytes) => {
                let pretty_bytes = String::from_utf8_lossy(bytes);
                f.debug_tuple("Inline").field(&pretty_bytes).finish()
            }
            JobOutputResult::Truncated { first, truncated } => {
                let pretty_first = String::from_utf8_lossy(first);
                f.debug_struct("Truncated")
                    .field("first", &pretty_first)
                    .field("truncated", truncated)
                    .finish()
            }
        }
    }
}

impl fmt::Display for JobOutputResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JobOutputResult::None => Ok(()),
            JobOutputResult::Inline(bytes) => fmt::Display::fmt(&String::from_utf8_lossy(bytes), f),
            JobOutputResult::Truncated { first, truncated } => {
                fmt::Display::fmt(&String::from_utf8_lossy(first), f)?;
                fmt::Display::fmt(&format!("<{truncated} bytes truncated>"), f)
            }
        }
    }
}

/// The output and duration of a job that ran for some amount of time. This is generated regardless
/// of how the job terminated. From our point of view, it doesn't matter. We ran the job until it
/// was terminated, and gathered its output.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct JobEffects {
    pub stdout: JobOutputResult,
    pub stderr: JobOutputResult,
    pub duration: Duration,
}

/// The outcome of a completed job. That is, a job that ran to completion, instead of timing out,
/// being canceled, etc.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct JobCompleted {
    pub status: JobStatus,
    pub effects: JobEffects,
}

/// The outcome of a job. This doesn't include error outcomes, which are handled with JobError.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum JobOutcome {
    Completed(JobCompleted),
    TimedOut(JobEffects),
}

/// A job failed to execute for some reason. We separate the universe of errors into "execution"
/// errors and "system" errors.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum JobError<T> {
    /// There was something wrong with the job that made it unable to be executed. This error
    /// indicates that there was something wrong with the job itself, and thus is obstensibly the
    /// fault of the client. An error of this type might happen if the execution path wasn't found, or
    /// if the binary couldn't be executed because it was for the wrong architecture.
    Execution(T),

    /// There was something wrong with the system that made it impossible to execute the job. There
    /// isn't anything different the client could do to mitigate this error. An error of this type
    /// might happen if the broker ran out of disk space, or there was a software error.
    System(T),
}

impl<T> JobError<T> {
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> JobError<U> {
        match self {
            JobError::Execution(e) => JobError::Execution(f(e)),
            JobError::System(e) => JobError::System(f(e)),
        }
    }
}

/// A common Result type in the worker.
pub type JobResult<T, E> = Result<T, JobError<E>>;

/// All relevant information about the outcome of a job. This is what's sent around between the
/// Worker, Broker, and Client.
pub type JobOutcomeResult = JobResult<JobOutcome, String>;

/// ID of a worker connection. These share the same ID space as [`ClientId`].
#[derive(
    Copy,
    Clone,
    Debug,
    Default,
    Deserialize,
    Display,
    Eq,
    From,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    Serialize,
)]
pub struct WorkerId(u32);

/// A SHA-256 digest.
#[derive(Clone, Constructor, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct Sha256Digest([u8; 32]);

impl Sha256Digest {
    /// Verify that two digests match. If not, return a [`Sha256DigestVerificationError`].
    pub fn verify(&self, expected: &Self) -> Result<(), Sha256DigestVerificationError> {
        if *self != *expected {
            Err(Sha256DigestVerificationError::new(
                self.clone(),
                expected.clone(),
            ))
        } else {
            Ok(())
        }
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_ref()
    }
}

#[derive(Debug)]
pub struct Sha256DigestTryFromError;

impl fmt::Display for Sha256DigestTryFromError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "failed to convert to SHA-256 digest")
    }
}

impl Error for Sha256DigestTryFromError {}

impl TryFrom<Vec<u8>> for Sha256Digest {
    type Error = Sha256DigestTryFromError;

    fn try_from(bytes: Vec<u8>) -> Result<Self, Self::Error> {
        Ok(Self(
            bytes.try_into().map_err(|_| Sha256DigestTryFromError)?,
        ))
    }
}

impl From<Sha256Digest> for Vec<u8> {
    fn from(d: Sha256Digest) -> Self {
        d.0.to_vec()
    }
}

impl From<u64> for Sha256Digest {
    fn from(input: u64) -> Self {
        let mut bytes = [0; 32];
        bytes[24..].copy_from_slice(&input.to_be_bytes());
        Sha256Digest(bytes)
    }
}

impl From<u32> for Sha256Digest {
    fn from(input: u32) -> Self {
        Sha256Digest::from(u64::from(input))
    }
}

impl FromStr for Sha256Digest {
    type Err = FromHexError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let mut bytes = [0; 32];
        hex::decode_to_slice(value, &mut bytes).map(|_| Sha256Digest(bytes))
    }
}

impl fmt::Display for Sha256Digest {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let mut bytes = [0; 64];
        hex::encode_to_slice(self.0, &mut bytes).unwrap();
        f.pad(unsafe { str::from_utf8_unchecked(&bytes) })
    }
}

impl Debug for Sha256Digest {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if f.alternate() {
            f.debug_tuple("Sha256Digest").field(&self.0).finish()
        } else {
            f.pad(&format!("Sha256Digest({})", self))
        }
    }
}

/// Error indicating that two digests that should have matched didn't.
#[derive(Debug)]
pub struct Sha256DigestVerificationError {
    pub actual: Sha256Digest,
    pub expected: Sha256Digest,
}

impl Sha256DigestVerificationError {
    pub fn new(actual: Sha256Digest, expected: Sha256Digest) -> Self {
        Sha256DigestVerificationError { actual, expected }
    }
}

impl fmt::Display for Sha256DigestVerificationError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "mismatched SHA-256 digest (expected {}, found {})",
            self.expected, self.actual,
        )
    }
}

impl Error for Sha256DigestVerificationError {}

#[cfg(test)]
mod tests {
    use super::*;
    use enumset::enum_set;
    use heck::ToKebabCase;
    use strum::IntoEnumIterator as _;

    #[test]
    fn client_id_display() {
        assert_eq!(format!("{}", ClientId::from(100)), "100");
        assert_eq!(format!("{}", ClientId::from(0)), "0");
        assert_eq!(format!("{:03}", ClientId::from(0)), "000");
        assert_eq!(format!("{:3}", ClientId::from(43)), " 43");
    }

    #[test]
    fn client_job_id_display() {
        assert_eq!(format!("{}", ClientJobId::from(100)), "100");
        assert_eq!(format!("{}", ClientJobId::from(0)), "0");
        assert_eq!(format!("{:03}", ClientJobId::from(0)), "000");
        assert_eq!(format!("{:3}", ClientJobId::from(43)), " 43");
    }

    #[test]
    fn user_id_display() {
        assert_eq!(format!("{}", UserId::from(100)), "100");
        assert_eq!(format!("{}", UserId::from(0)), "0");
        assert_eq!(format!("{:03}", UserId::from(0)), "000");
        assert_eq!(format!("{:3}", UserId::from(43)), " 43");
    }

    #[test]
    fn group_id_display() {
        assert_eq!(format!("{}", GroupId::from(100)), "100");
        assert_eq!(format!("{}", GroupId::from(0)), "0");
        assert_eq!(format!("{:03}", GroupId::from(0)), "000");
        assert_eq!(format!("{:3}", GroupId::from(43)), " 43");
    }

    #[test]
    fn worker_id_display() {
        assert_eq!(format!("{}", WorkerId::from(100)), "100");
        assert_eq!(format!("{}", WorkerId::from(0)), "0");
        assert_eq!(format!("{:03}", WorkerId::from(0)), "000");
        assert_eq!(format!("{:3}", WorkerId::from(43)), " 43");
    }

    #[test]
    fn from_u32() {
        assert_eq!(
            Sha256Digest::from(0x12345678u32),
            Sha256Digest([
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0x12, 0x34, 0x56, 0x78,
            ])
        );
    }

    #[test]
    fn from_u64() {
        assert_eq!(
            Sha256Digest::from(0x123456789ABCDEF0u64),
            Sha256Digest([
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x12, 0x34,
                0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0,
            ])
        );
    }

    #[test]
    fn from_str_ok() {
        assert_eq!(
            "101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f"
                .parse::<Sha256Digest>()
                .unwrap(),
            Sha256Digest([
                0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d,
                0x1e, 0x1f, 0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2a, 0x2b,
                0x2c, 0x2d, 0x2e, 0x2f,
            ])
        );
    }

    #[test]
    fn from_str_wrong_length() {
        assert_eq!(
            "101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f0"
                .parse::<Sha256Digest>()
                .unwrap_err(),
            FromHexError::OddLength
        );
        assert_eq!(
            "101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f0f"
                .parse::<Sha256Digest>()
                .unwrap_err(),
            FromHexError::InvalidStringLength
        );
        assert_eq!(
            "101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e"
                .parse::<Sha256Digest>()
                .unwrap_err(),
            FromHexError::InvalidStringLength
        );
    }

    #[test]
    fn from_str_bad_chars() {
        assert_eq!(
            " 01112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f"
                .parse::<Sha256Digest>()
                .unwrap_err(),
            FromHexError::InvalidHexCharacter { c: ' ', index: 0 }
        );
        assert_eq!(
            "101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2g"
                .parse::<Sha256Digest>()
                .unwrap_err(),
            FromHexError::InvalidHexCharacter { c: 'g', index: 63 }
        );
    }

    #[test]
    fn display_round_trip() {
        let s = "101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f";
        assert_eq!(s, s.parse::<Sha256Digest>().unwrap().to_string());
    }

    #[test]
    fn display_padding() {
        let d = "101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f"
            .parse::<Sha256Digest>()
            .unwrap();
        assert_eq!(
            format!("{d:<70}"),
            "101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f      "
        );
        assert_eq!(
            format!("{d:0>70}"),
            "000000101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f"
        );
    }

    #[test]
    fn debug() {
        let d = "101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f"
            .parse::<Sha256Digest>()
            .unwrap();
        assert_eq!(
            format!("{d:?}"),
            "Sha256Digest(101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f)"
        );
        assert_eq!(
            format!("{d:80?}"),
            "Sha256Digest(101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f)  "
        );
        assert_eq!(
            format!("{d:#?}"),
            "Sha256Digest(
    [
        16,
        17,
        18,
        19,
        20,
        21,
        22,
        23,
        24,
        25,
        26,
        27,
        28,
        29,
        30,
        31,
        32,
        33,
        34,
        35,
        36,
        37,
        38,
        39,
        40,
        41,
        42,
        43,
        44,
        45,
        46,
        47,
    ],
)"
        );
    }

    #[test]
    fn job_spec_must_be_run_locally_network() {
        let spec = JobSpec::new(
            "foo",
            nonempty![(Sha256Digest::from(0u32), ArtifactType::Tar)],
        );
        assert_eq!(spec.must_be_run_locally(), false);

        let spec = spec.network(JobNetwork::Loopback);
        assert_eq!(spec.must_be_run_locally(), false);

        let spec = spec.network(JobNetwork::Local);
        assert_eq!(spec.must_be_run_locally(), true);

        let spec = spec.network(JobNetwork::Disabled);
        assert_eq!(spec.must_be_run_locally(), false);
    }

    #[test]
    fn job_spec_must_be_run_locally_mounts() {
        let spec = JobSpec::new(
            "foo",
            nonempty![(Sha256Digest::from(0u32), ArtifactType::Tar)],
        );
        assert_eq!(spec.must_be_run_locally(), false);

        let spec = spec.mounts([
            JobMount::Sys {
                mount_point: Utf8PathBuf::from("/sys"),
            },
            JobMount::Bind {
                mount_point: Utf8PathBuf::from("/bind"),
                local_path: Utf8PathBuf::from("/a"),
                read_only: false,
            },
        ]);
        assert_eq!(spec.must_be_run_locally(), true);

        let spec = spec.mounts([]);
        assert_eq!(spec.must_be_run_locally(), false);
    }

    #[test]
    fn job_spec_must_be_run_locally_root_overlay() {
        let spec = JobSpec::new(
            "foo",
            nonempty![(Sha256Digest::from(0u32), ArtifactType::Tar)],
        );
        assert_eq!(spec.must_be_run_locally(), false);

        let spec = spec.root_overlay(JobRootOverlay::None);
        assert_eq!(spec.must_be_run_locally(), false);

        let spec = spec.root_overlay(JobRootOverlay::Tmp);
        assert_eq!(spec.must_be_run_locally(), false);

        let spec = spec.root_overlay(JobRootOverlay::Local {
            upper: "upper".into(),
            work: "work".into(),
        });
        assert_eq!(spec.must_be_run_locally(), true);
    }

    #[test]
    fn job_spec_must_be_run_locally_allocate_tty() {
        let spec = JobSpec::new(
            "foo",
            nonempty![(Sha256Digest::from(0u32), ArtifactType::Tar)],
        );
        assert_eq!(spec.must_be_run_locally(), false);

        let spec = spec.allocate_tty(Some(JobTty::new(b"\0abcde", WindowSize::new(20, 80))));
        assert_eq!(spec.must_be_run_locally(), true);

        let spec = spec.allocate_tty(None::<JobTty>);
        assert_eq!(spec.must_be_run_locally(), false);
    }

    trait AssertError {
        fn assert_error(&self, expected: &str);
    }

    impl AssertError for toml::de::Error {
        fn assert_error(&self, expected: &str) {
            let message = self.message();
            assert!(message.starts_with(expected), "message: {message}");
        }
    }

    fn deserialize_value<T: for<'a> Deserialize<'a>>(file: &str) -> T {
        T::deserialize(toml::de::ValueDeserializer::new(file)).unwrap()
    }

    fn deserialize_value_error<T: for<'a> Deserialize<'a> + Debug>(file: &str) -> toml::de::Error {
        match T::deserialize(toml::de::ValueDeserializer::new(file)) {
            Err(err) => err,
            Ok(val) => panic!("expected a toml error but instead got value: {val:?}"),
        }
    }

    #[test]
    fn enumset_job_device_for_toml_and_json_deserialized_as_list() {
        let devices: EnumSet<JobDeviceForTomlAndJson> = deserialize_value(r#"["full", "null"]"#);
        let devices: EnumSet<_> = devices.into_iter().map(Into::<JobDevice>::into).collect();
        assert_eq!(devices, enum_set!(JobDevice::Full | JobDevice::Null));
    }

    #[test]
    fn enumset_job_device_for_toml_and_json_deserialize_unknown_field() {
        deserialize_value_error::<EnumSet<JobDeviceForTomlAndJson>>(r#"["bull", "null"]"#)
            .assert_error("unknown variant `bull`");
    }

    #[test]
    fn job_device_for_toml_and_json_and_job_device_match() {
        for job_device in JobDevice::iter() {
            let repr = format!(r#""{}""#, format!("{job_device:?}").to_kebab_case());
            assert_eq!(
                JobDevice::from(deserialize_value::<JobDeviceForTomlAndJson>(&repr)),
                job_device
            );
        }
    }

    #[test]
    fn bind_mount_no_read_only() {
        let job_mount: JobMountForTomlAndJson =
            deserialize_value(r#"{ type = "bind", mount_point = "/mnt", local_path = "/a" }"#);
        let job_mount: JobMount = job_mount.into();
        assert_eq!(
            job_mount,
            JobMount::Bind {
                mount_point: Utf8PathBuf::from("/mnt"),
                local_path: Utf8PathBuf::from("/a"),
                read_only: false,
            }
        );
    }
}
