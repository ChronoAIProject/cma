//! The media job's command line: the operations a job may run, how the
//! gateway launches the wrapper, how the wrapper parses its arguments, and the
//! exact FFmpeg or ffprobe argv each operation executes.
//!
//! Everything here is pure, so the argv every platform's tests pin is the argv
//! the Linux wrapper executes.
use std::{
    ffi::{OsStr, OsString},
    fmt,
    path::{Path, PathBuf},
};

/// The latest keyframe position accepted, in milliseconds: one day.
const MAX_FRAME_MILLIS: u64 = 86_400_000;
/// The longest transcode, in seconds; above `CMAEG_STT_MAX_SECONDS`'s bound.
const MAX_AUDIO_SECONDS: u16 = 1_500;
/// The wrapper's flag for one media job.
pub const MEDIA_JOB_FLAG: &str = "--media-job";
/// The wrapper's flag for the sandbox self-check.
pub const SANDBOX_CHECK_FLAG: &str = "--media-sandbox-check";

// The protocol whitelist applies to the input only; the output `pipe:1` uses
// the compiled-in `file` and `pipe` protocols. There is no `-fs`: the gateway
// caps each job's stdout instead, and every job writes only to stdout.
/// Containers a video or animation may arrive in.
const VIDEO_FORMATS: &str = "mov,mp4,m4a,3gp,matroska,webm,gif";
/// The demuxers of the configure line, so Telegram voice and Lark Opus are
/// admitted as well as audio tracks of video.
const AUDIO_FORMATS: &str = "mov,mp4,m4a,3gp,matroska,webm,ogg,mp3,wav,flac,aac,amr";
/// 4K: larger frames are refused before decoding.
const MAX_PIXELS: &str = "8294400";

/// What one media job does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobOp {
    /// Read the container's duration and stream types.
    Probe,
    /// One JPEG keyframe at or after a position.
    Frame(FrameTime),
    /// Up to N seconds of 16 kHz mono PCM.
    Audio(AudioSeconds),
    /// Not a media job: the sandbox self-check, launched through the same
    /// spawn, pidfd, watchdog, reap and classify path as a job.
    SandboxCheck,
}

impl JobOp {
    /// The metric and span label.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Probe => "probe",
            Self::Frame(_) => "frame",
            Self::Audio(_) => "audio",
            Self::SandboxCheck => "sandbox_check",
        }
    }
}

/// A keyframe position, `^[0-9]{1,5}\.[0-9]{3}$` and at most `86400.000`.
#[derive(Clone, PartialEq, Eq)]
pub struct FrameTime(String);

impl FrameTime {
    pub fn parse(value: &str) -> Result<Self, JobArgError> {
        let (whole, fraction) = value.split_once('.').ok_or(JobArgError::InvalidFrameTime)?;
        let digits = |part: &str, range: std::ops::RangeInclusive<usize>| {
            range.contains(&part.len()) && part.bytes().all(|byte| byte.is_ascii_digit())
        };
        if !digits(whole, 1..=5) || !digits(fraction, 3..=3) {
            return Err(JobArgError::InvalidFrameTime);
        }
        // At most 99999 and 999: the arithmetic cannot overflow.
        let millis = whole.parse::<u64>().unwrap_or(u64::MAX) * 1_000
            + fraction.parse::<u64>().unwrap_or(u64::MAX);
        if millis > MAX_FRAME_MILLIS {
            return Err(JobArgError::InvalidFrameTime);
        }
        Ok(Self(value.to_owned()))
    }

    /// The position `millis` milliseconds in, formatted with three decimals.
    pub fn from_millis(millis: u64) -> Result<Self, JobArgError> {
        Self::parse(&format!("{}.{:03}", millis / 1_000, millis % 1_000))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for FrameTime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A transcode length in `1..=1500` seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioSeconds(u16);

impl AudioSeconds {
    pub fn new(seconds: u16) -> Result<Self, JobArgError> {
        if (1..=MAX_AUDIO_SECONDS).contains(&seconds) {
            Ok(Self(seconds))
        } else {
            Err(JobArgError::InvalidAudioSeconds)
        }
    }

    pub fn get(&self) -> u16 {
        self.0
    }
}

/// A parsed `--media-job` invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobInvocation {
    pub op: JobOp,
    pub bin_dir: PathBuf,
    pub job_dir: PathBuf,
}

/// Why the wrapper refused its arguments. It exits 125 on any of these
/// without executing anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobArgError {
    UnknownOp,
    MissingArgument,
    ExtraArgument,
    NotUtf8,
    RelativePath,
    InvalidFrameTime,
    InvalidAudioSeconds,
}

/// Parse `<op> <bin-dir> <job-dir> [arg]`, the arguments after
/// `--media-job`. Only `probe`, `frame` and `audio` are media jobs; any other
/// token, `sandbox-check` included, is refused.
pub fn parse_job_args(args: &[OsString]) -> Result<JobInvocation, JobArgError> {
    let [op, bin_dir, job_dir, rest @ ..] = args else {
        return Err(JobArgError::MissingArgument);
    };
    let bin_dir = absolute(bin_dir)?;
    let job_dir = absolute(job_dir)?;
    let op = match (utf8(op)?, rest) {
        ("probe", []) => JobOp::Probe,
        ("frame", [time]) => JobOp::Frame(FrameTime::parse(utf8(time)?)?),
        ("audio", [seconds]) => {
            let seconds = utf8(seconds)?;
            // `u16::from_str` admits a leading `+`; the argument is digits only.
            if seconds.is_empty() || !seconds.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(JobArgError::InvalidAudioSeconds);
            }
            let value = seconds
                .parse::<u16>()
                .map_err(|_| JobArgError::InvalidAudioSeconds)?;
            JobOp::Audio(AudioSeconds::new(value)?)
        }
        ("probe", _) => return Err(JobArgError::ExtraArgument),
        ("frame" | "audio", []) => return Err(JobArgError::MissingArgument),
        ("frame" | "audio", _) => return Err(JobArgError::ExtraArgument),
        _ => return Err(JobArgError::UnknownOp),
    };
    Ok(JobInvocation {
        op,
        bin_dir,
        job_dir,
    })
}

fn utf8(value: &OsStr) -> Result<&str, JobArgError> {
    value.to_str().ok_or(JobArgError::NotUtf8)
}

fn absolute(value: &OsStr) -> Result<PathBuf, JobArgError> {
    let path = Path::new(utf8(value)?);
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Err(JobArgError::RelativePath)
    }
}

/// The gateway's own arguments for running `op`: the self-check takes no
/// directory, every media job names the binary and job directories.
pub fn launch_args(op: &JobOp, bin_dir: &Path, job_dir: &Path) -> Vec<OsString> {
    let token = match op {
        JobOp::SandboxCheck => return vec![SANDBOX_CHECK_FLAG.into()],
        JobOp::Probe => "probe",
        JobOp::Frame(_) => "frame",
        JobOp::Audio(_) => "audio",
    };
    let mut args: Vec<OsString> = vec![
        MEDIA_JOB_FLAG.into(),
        token.into(),
        bin_dir.into(),
        job_dir.into(),
    ];
    match op {
        JobOp::Frame(time) => args.push(time.as_str().into()),
        JobOp::Audio(seconds) => args.push(seconds.get().to_string().into()),
        JobOp::Probe | JobOp::SandboxCheck => {}
    }
    args
}

/// The program `op` executes and its exact argv, argv[0] included. The
/// self-check executes nothing.
pub fn program(op: &JobOp) -> Option<(&'static str, Vec<String>)> {
    let owned = |args: &[&str]| args.iter().map(|arg| (*arg).to_owned()).collect();
    match op {
        JobOp::SandboxCheck => None,
        JobOp::Probe => Some((
            "ffprobe",
            owned(&[
                "ffprobe",
                "-v",
                "error",
                "-hide_banner",
                "-protocol_whitelist",
                "file",
                "-format_whitelist",
                VIDEO_FORMATS,
                "-show_entries",
                "format=duration:stream=codec_type",
                "-of",
                "json",
                "input",
            ]),
        )),
        JobOp::Frame(time) => {
            let mut args: Vec<String> = owned(&decode_prefix(VIDEO_FORMATS));
            args.extend(owned(&[
                "-ss",
                time.as_str(),
                "-skip_frame",
                "nokey",
                "-i",
                "input",
                "-frames:v",
                "1",
                "-vf",
                "scale=768:768:force_original_aspect_ratio=decrease",
                "-c:v",
                "mjpeg",
                "-q:v",
                "5",
                "-f",
                "image2pipe",
                "pipe:1",
            ]));
            Some(("ffmpeg", args))
        }
        JobOp::Audio(seconds) => {
            let seconds = seconds.get().to_string();
            let mut args: Vec<String> = owned(&decode_prefix(AUDIO_FORMATS));
            args.extend(owned(&[
                "-i",
                "input",
                "-vn",
                "-ac",
                "1",
                "-ar",
                "16000",
                "-t",
                seconds.as_str(),
                "-c:a",
                "pcm_s16le",
                "-f",
                "s16le",
                "pipe:1",
            ]));
            Some(("ffmpeg", args))
        }
    }
}

/// The options every ffmpeg job starts with.
fn decode_prefix(formats: &'static str) -> [&'static str; 13] {
    [
        "ffmpeg",
        "-nostdin",
        "-hide_banner",
        "-loglevel",
        "error",
        "-threads",
        "1",
        "-protocol_whitelist",
        "file",
        "-format_whitelist",
        formats,
        "-max_pixels",
        MAX_PIXELS,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    fn argv(op: &JobOp) -> String {
        let (name, argv) = program(op).unwrap();
        assert_eq!(argv[0], name);
        argv.join(" ")
    }

    #[test]
    fn each_op_executes_exactly_its_argv() {
        assert_eq!(
            argv(&JobOp::Probe),
            "ffprobe -v error -hide_banner -protocol_whitelist file -format_whitelist mov,mp4,m4a,3gp,matroska,webm,gif -show_entries format=duration:stream=codec_type -of json input"
        );
        assert_eq!(
            argv(&JobOp::Frame(FrameTime::parse("2.500").unwrap())),
            "ffmpeg -nostdin -hide_banner -loglevel error -threads 1 -protocol_whitelist file -format_whitelist mov,mp4,m4a,3gp,matroska,webm,gif -max_pixels 8294400 -ss 2.500 -skip_frame nokey -i input -frames:v 1 -vf scale=768:768:force_original_aspect_ratio=decrease -c:v mjpeg -q:v 5 -f image2pipe pipe:1"
        );
        assert_eq!(
            argv(&JobOp::Audio(AudioSeconds::new(600).unwrap())),
            "ffmpeg -nostdin -hide_banner -loglevel error -threads 1 -protocol_whitelist file -format_whitelist mov,mp4,m4a,3gp,matroska,webm,ogg,mp3,wav,flac,aac,amr -max_pixels 8294400 -i input -vn -ac 1 -ar 16000 -t 600 -c:a pcm_s16le -f s16le pipe:1"
        );
        assert_eq!(program(&JobOp::SandboxCheck), None);
    }

    #[test]
    fn launch_args_name_the_op_and_both_directories() {
        let (bin, job) = (Path::new("/opt/ff"), Path::new("/var/cmaeg/media/a1"));
        assert_eq!(
            launch_args(&JobOp::Probe, bin, job),
            args(&["--media-job", "probe", "/opt/ff", "/var/cmaeg/media/a1"])
        );
        assert_eq!(
            launch_args(&JobOp::Frame(FrameTime::parse("0.000").unwrap()), bin, job),
            args(&[
                "--media-job",
                "frame",
                "/opt/ff",
                "/var/cmaeg/media/a1",
                "0.000"
            ])
        );
        assert_eq!(
            launch_args(&JobOp::Audio(AudioSeconds::new(1).unwrap()), bin, job),
            args(&[
                "--media-job",
                "audio",
                "/opt/ff",
                "/var/cmaeg/media/a1",
                "1"
            ])
        );
        assert_eq!(
            launch_args(&JobOp::SandboxCheck, bin, job),
            args(&["--media-sandbox-check"])
        );
    }

    #[test]
    fn launch_args_parse_back_into_the_same_job() {
        let (bin, job) = (Path::new("/opt/ff"), Path::new("/var/cmaeg/media/a1"));
        for op in [
            JobOp::Probe,
            JobOp::Frame(FrameTime::parse("86400.000").unwrap()),
            JobOp::Audio(AudioSeconds::new(1_500).unwrap()),
        ] {
            let launched = launch_args(&op, bin, job);
            assert_eq!(
                parse_job_args(&launched[1..]),
                Ok(JobInvocation {
                    op,
                    bin_dir: bin.into(),
                    job_dir: job.into(),
                })
            );
        }
    }

    #[test]
    fn frame_times_are_validated() {
        for valid in ["0.000", "9.167", "00001.250", "86400.000"] {
            assert_eq!(FrameTime::parse(valid).unwrap().as_str(), valid);
        }
        for invalid in [
            "99999.000",
            "1.5",
            "86400.001",
            "123456.000",
            "-1.000",
            "1e3.000",
            "",
            ".500",
            "1.0000",
            "1,000",
            " 1.000",
            "+1.000",
        ] {
            assert_eq!(
                FrameTime::parse(invalid),
                Err(JobArgError::InvalidFrameTime),
                "{invalid:?}"
            );
        }
        assert_eq!(FrameTime::from_millis(833).unwrap().as_str(), "0.833");
        assert_eq!(FrameTime::from_millis(9_167).unwrap().as_str(), "9.167");
        assert!(FrameTime::from_millis(86_400_001).is_err());
    }

    #[test]
    fn rejected_invocations() {
        let dirs = ["/opt/ff", "/var/cmaeg/media/a1"];
        let with = |op: &str, extra: &[&str]| {
            let mut values = vec![op, dirs[0], dirs[1]];
            values.extend_from_slice(extra);
            parse_job_args(&args(&values))
        };
        assert_eq!(with("frame", &["1.5"]), Err(JobArgError::InvalidFrameTime));
        assert_eq!(
            with("frame", &["86400.001"]),
            Err(JobArgError::InvalidFrameTime)
        );
        assert_eq!(
            with("frame", &["123456.000"]),
            Err(JobArgError::InvalidFrameTime)
        );
        assert_eq!(
            with("frame", &["-1.000"]),
            Err(JobArgError::InvalidFrameTime)
        );
        assert_eq!(
            with("frame", &["1e3.000"]),
            Err(JobArgError::InvalidFrameTime)
        );
        for seconds in ["0", "1501", "+5", "-1", "", "65536", "1.0"] {
            assert_eq!(
                with("audio", &[seconds]),
                Err(JobArgError::InvalidAudioSeconds),
                "{seconds:?}"
            );
        }
        assert_eq!(with("transcode", &[]), Err(JobArgError::UnknownOp));
        assert_eq!(with("sandbox-check", &[]), Err(JobArgError::UnknownOp));
        assert_eq!(with("probe", &["extra"]), Err(JobArgError::ExtraArgument));
        assert_eq!(
            with("frame", &["1.000", "2.000"]),
            Err(JobArgError::ExtraArgument)
        );
        assert_eq!(with("frame", &[]), Err(JobArgError::MissingArgument));
        assert_eq!(with("audio", &[]), Err(JobArgError::MissingArgument));
        assert_eq!(
            parse_job_args(&args(&["probe", "/opt/ff"])),
            Err(JobArgError::MissingArgument)
        );
        assert_eq!(parse_job_args(&[]), Err(JobArgError::MissingArgument));
        assert_eq!(
            parse_job_args(&args(&["probe", "opt/ff", dirs[1]])),
            Err(JobArgError::RelativePath)
        );
        assert_eq!(
            parse_job_args(&args(&["probe", dirs[0], "media/a1"])),
            Err(JobArgError::RelativePath)
        );
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_arguments_are_rejected() {
        use std::os::unix::ffi::OsStringExt;
        let invalid = OsString::from_vec(vec![b'/', 0xff, b'x']);
        let valid = OsString::from("/opt/ff");
        assert_eq!(
            parse_job_args(&[OsString::from("probe"), valid.clone(), invalid.clone()]),
            Err(JobArgError::NotUtf8)
        );
        assert_eq!(
            parse_job_args(&[invalid.clone(), valid.clone(), valid.clone()]),
            Err(JobArgError::NotUtf8)
        );
        assert_eq!(
            parse_job_args(&[OsString::from("frame"), valid.clone(), valid, invalid]),
            Err(JobArgError::NotUtf8)
        );
    }
}
