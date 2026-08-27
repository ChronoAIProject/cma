//! Files a kernel needs on disk before it starts.
//!
//! The supervisor hosts any kernel and knows none, so it does not decide what
//! a kernel needs written or where — it is told both, writes what it is
//! handed, and reads neither. That is the same naivety the conversation carry
//! commits to, for the same reason: a kernel that changes its own format must
//! not be able to break the pod hosting it.
//!
//! This existed before as one kernel's configuration file, rendered here in
//! that kernel's own vocabulary at that kernel's own path. It worked while
//! there was one kernel. With three it wrote a file two of them do not read,
//! naming a provider route they do not use, into every one of their pods.
//!
//! A kernel that needs nothing written says nothing and nothing is written.

use std::{
    io,
    marker::PhantomData,
    path::{Component, Path, PathBuf},
};

use serde::de::{self, SeqAccess, Visitor};

#[cfg(unix)]
use std::{
    ffi::{CString, OsStr},
    io::Write,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    os::unix::ffi::OsStrExt,
    sync::atomic::{AtomicU64, Ordering},
};

/// Where the file goes, relative to the pod's home, and what goes in it.
///
/// Two variables rather than one structured value because the adapter that
/// sets them is composing an environment, and a path and a body are the two
/// things it has to say. Absent means this kernel needs no file.
pub const PATH_VAR: &str = "CHRONO_RUNTIME_CONFIG_PATH";

/// The file's content, verbatim.
pub const BODY_VAR: &str = "CHRONO_RUNTIME_CONFIG";

/// Write a kernel's configuration, if it was given one.
///
/// Returns where it was written, or `None` when nothing was asked for. A
/// failure is an error rather than a warning: a kernel started without the
/// configuration it was meant to have is a session that runs differently from
/// the one that was ordered, and silently.
pub fn write(home: &Path) -> std::io::Result<Option<PathBuf>> {
    let (Ok(relative), Ok(body)) = (std::env::var(PATH_VAR), std::env::var(BODY_VAR)) else {
        return Ok(None);
    };
    if relative.trim().is_empty() {
        return Ok(None);
    }
    let Some(relative) = contained(&relative) else {
        return Err(std::io::Error::other(format!(
            "{PATH_VAR} must be a path inside the pod's home, and {relative:?} is not"
        )));
    };
    AnchoredHome::open(home)?.write(&relative, body.as_bytes())?;
    let path = home.join(relative);
    // Never with its content: what a kernel is configured with is between the
    // adapter that composed it and the agent that reads it, and it names an
    // endpoint this pod is proxying for somebody.
    tracing::info!(path = %path.display(), bytes = body.len(), "kernel configuration written");
    Ok(Some(path))
}

/// Version of the framing named by `chronoPrelude`.
const PACK_VERSION: u32 = 1;

/// The largest pack this supervisor will take.
///
/// A bound rather than a guess: the two skills a session carries by default
/// are about 400 KB together, and the cap is an order above that so a bigger
/// selection still lands while a runaway body is refused before it is read
/// into memory.
pub const PACK_LIMIT: usize = 4 * 1024 * 1024;
/// Skill files plus the optional standing-instructions document.
const PACK_FILE_LIMIT: usize = 4_097;
/// Largest byte length of one component accepted by the Linux runtime image.
const PACK_COMPONENT_LIMIT: usize = 255;

/// Whether this build can safely write a runtime prelude beneath its home.
pub const SUPPORTED: bool = cfg!(unix);

#[derive(serde::Deserialize)]
struct PackHeader {
    #[serde(rename = "chronoPrelude")]
    format: u32,
    #[serde(deserialize_with = "deserialize_files")]
    files: Vec<PackEntry>,
}

#[derive(serde::Deserialize)]
struct PackEntry {
    path: String,
    len: u64,
}

fn deserialize_files<'de, D>(deserializer: D) -> Result<Vec<PackEntry>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserializer.deserialize_seq(BoundedFilesVisitor { entry: PhantomData })
}

struct BoundedFilesVisitor<T> {
    entry: PhantomData<T>,
}

impl<'de> Visitor<'de> for BoundedFilesVisitor<PackEntry> {
    type Value = Vec<PackEntry>;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a bounded list of runtime prelude files")
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        if sequence
            .size_hint()
            .is_some_and(|length| length > PACK_FILE_LIMIT)
        {
            return Err(de::Error::custom("runtime prelude declares too many files"));
        }
        let mut entries = Vec::with_capacity(
            sequence
                .size_hint()
                .unwrap_or_default()
                .min(PACK_FILE_LIMIT),
        );
        loop {
            if entries.len() == PACK_FILE_LIMIT {
                return match sequence.next_element::<de::IgnoredAny>()? {
                    Some(_) => Err(de::Error::custom("runtime prelude declares too many files")),
                    None => Ok(entries),
                };
            }
            match sequence.next_element()? {
                Some(entry) => entries.push(entry),
                None => return Ok(entries),
            }
        }
    }
}

/// Files a kernel needs, carried in one body.
///
/// A JSON header line naming each path and its length, then the bodies
/// concatenated in that order. Not a tar: every entry needs the same
/// relative-path check either way, and a tar brings symlinks, hardlinks,
/// device nodes and modes that all have to be refused — a dependency to
/// express a header this module already knows how to write.
///
/// The paths are composed by whoever sends this, because where a kernel keeps
/// its files is that kernel's business and this process hosts any of them.
pub fn unpack(body: &[u8]) -> Result<Vec<(PathBuf, Vec<u8>)>, String> {
    if body.len() > PACK_LIMIT {
        return Err(format!("a prelude may be at most {PACK_LIMIT} bytes"));
    }
    let split = body
        .iter()
        .position(|b| *b == b'\n')
        .ok_or_else(|| "a prelude begins with a header line".to_string())?;
    let header: PackHeader = serde_json::from_slice(&body[..split])
        .map_err(|e| format!("the prelude header is not usable: {e}"))?;
    if header.format != PACK_VERSION {
        return Err("this is not a prelude this supervisor understands".to_string());
    }

    let mut at = split + 1;
    let mut files = Vec::with_capacity(header.files.len());
    for entry in header.files {
        let raw = entry.path;
        let len = usize::try_from(entry.len)
            .map_err(|_| format!("{raw} declares a length this platform cannot hold"))?;
        // Checked before reading, so a declared length longer than the body
        // is refused rather than panicking on the slice.
        let end = at
            .checked_add(len)
            .filter(|end| *end <= body.len())
            .ok_or_else(|| format!("{raw} declares more bytes than the prelude carries"))?;
        let path =
            contained(&raw).ok_or_else(|| format!("{raw} is not a path inside the pod's home"))?;
        if raw.chars().any(char::is_control) || path.as_os_str() != Path::new(&raw).as_os_str() {
            return Err(format!(
                "{raw} is not a canonical path inside the pod's home"
            ));
        }
        files.push((path, body[at..end].to_vec()));
        at = end;
    }
    validate_targets(&files)?;
    if at != body.len() {
        return Err("the runtime prelude has unclaimed trailing bytes".to_string());
    }
    Ok(files)
}

fn validate_targets(files: &[(PathBuf, Vec<u8>)]) -> Result<(), String> {
    // Sorting borrowed paths by component makes equal targets and file/child
    // conflicts adjacent without retaining a copy of every deep prefix.
    let mut targets = files
        .iter()
        .map(|(path, _)| path.as_path())
        .collect::<Vec<_>>();
    targets.sort_unstable_by(|left, right| left.components().cmp(right.components()));
    for pair in targets.windows(2) {
        if pair[0] == pair[1] {
            return Err(format!(
                "{} repeats a runtime prelude target",
                pair[1].display()
            ));
        }
        if is_component_prefix(pair[0], pair[1]) {
            return Err(format!(
                "{} conflicts with another runtime prelude file or directory target",
                pair[1].display()
            ));
        }
    }
    Ok(())
}

fn is_component_prefix(prefix: &Path, path: &Path) -> bool {
    let mut components = path.components();
    prefix
        .components()
        .all(|component| components.next() == Some(component))
}

/// Write files a kernel needs, and say how many landed.
///
/// All or nothing is deliberately **not** attempted. A partial write is
/// visible — the kernel finds some of what it was promised — and unwinding
/// would mean deleting files this process did not create. The caller is told
/// which entry failed and decides.
pub fn write_all(home: &Path, files: &[(PathBuf, Vec<u8>)]) -> std::io::Result<usize> {
    let home = (!files.is_empty())
        .then(|| AnchoredHome::open(home))
        .transpose()?;
    for (relative, body) in files {
        home.as_ref()
            .expect("a non-empty prelude opened its home")
            .write(relative, body)?;
    }
    // Never the contents: what a kernel is given is between the adapter that
    // composed it and the agent that reads it.
    tracing::info!(files = files.len(), "prelude files written");
    Ok(files.len())
}

/// Write one file relative to an already-open directory authority.
///
/// Every path component is resolved from a held directory descriptor and with
/// symlink following disabled. A metadata check before an ordinary path write
/// would leave a rename window between the check and the write; `openat` keeps
/// lookup and use in one kernel operation instead.
#[cfg(unix)]
struct AnchoredHome {
    directory: OwnedFd,
}

#[cfg(unix)]
impl AnchoredHome {
    fn open(home: &Path) -> io::Result<Self> {
        open_home(home).map(|directory| Self { directory })
    }

    fn write(&self, relative: &Path, body: &[u8]) -> io::Result<()> {
        let mut directory = self.directory.try_clone()?;
        let mut components = relative.components().peekable();

        while let Some(component) = components.next() {
            let Component::Normal(name) = component else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "a runtime prelude target must be a non-empty relative path",
                ));
            };
            if components.peek().is_some() {
                directory = open_or_create_directory(&directory, name)?;
                continue;
            }
            return replace_file(&directory, name, body);
        }

        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a runtime prelude target must name a file",
        ))
    }
}

#[cfg(unix)]
fn open_home(home: &Path) -> io::Result<OwnedFd> {
    let path = CString::new(home.as_os_str().as_bytes()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "the runtime home contains a null byte",
        )
    })?;
    // SAFETY: `path` is NUL-terminated and remains alive for the call. The
    // returned descriptor is immediately given unique ownership below.
    let descriptor = unsafe {
        libc::open(
            path.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    owned_descriptor(descriptor)
}

#[cfg(unix)]
fn open_or_create_directory(parent: &OwnedFd, name: &OsStr) -> io::Result<OwnedFd> {
    let name = component_name(name)?;
    match open_directory(parent, &name) {
        Ok(directory) => Ok(directory),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            // SAFETY: `name` is NUL-terminated, and `parent` remains open for
            // both this operation and the no-follow open that verifies it.
            let created = unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o777) };
            if created == -1 {
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::AlreadyExists {
                    return Err(error);
                }
            }
            // Another process may have won creation or replaced the name. Only
            // this descriptor-producing no-follow lookup decides what we use.
            open_directory(parent, &name)
        }
        Err(error) => Err(error),
    }
}

#[cfg(unix)]
fn open_directory(parent: &OwnedFd, name: &CString) -> io::Result<OwnedFd> {
    // SAFETY: `name` is NUL-terminated and `parent` is a live directory
    // descriptor. Ownership of a successful result is transferred once.
    let descriptor = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    owned_descriptor(descriptor)
}

#[cfg(unix)]
fn replace_file(parent: &OwnedFd, name: &OsStr, body: &[u8]) -> io::Result<()> {
    let name = component_name(name)?;
    refuse_existing_symlink(parent, &name)?;

    let mut staged = StagedFile::create(parent)?;
    staged.file.write_all(body)?;
    staged.commit(&name)
}

/// A same-directory file that replaces its target only after its body lands.
///
/// Replacing the directory entry rather than truncating what it names matters
/// for hardlinks: a pre-existing target may share its inode with a file outside
/// the runtime home. The new inode severs that alias without ever opening it.
#[cfg(unix)]
struct StagedFile<'a> {
    parent: &'a OwnedFd,
    name: CString,
    file: std::fs::File,
    committed: bool,
}

#[cfg(unix)]
impl<'a> StagedFile<'a> {
    fn create(parent: &'a OwnedFd) -> io::Result<Self> {
        static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(0);
        const ATTEMPTS: usize = 128;

        for _ in 0..ATTEMPTS {
            let sequence = NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed);
            let name = CString::new(format!(
                ".cma-prelude-{}-{sequence}.tmp",
                std::process::id()
            ))
            .expect("the temporary prelude name has no null byte");
            // The mode matches a newly created `std::fs::write` target and is
            // still reduced by the process umask. `O_EXCL` ensures an existing
            // file, hardlink, or symlink can neither be opened nor followed.
            // SAFETY: `name` is NUL-terminated and `parent` is a live directory
            // descriptor. A successful descriptor is uniquely owned below.
            let descriptor = unsafe {
                libc::openat(
                    parent.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_WRONLY
                        | libc::O_CREAT
                        | libc::O_EXCL
                        | libc::O_CLOEXEC
                        | libc::O_NOFOLLOW,
                    0o666,
                )
            };
            match owned_descriptor(descriptor) {
                Ok(descriptor) => {
                    return Ok(Self {
                        parent,
                        name,
                        file: descriptor.into(),
                        committed: false,
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }

        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "cannot reserve a temporary runtime prelude file",
        ))
    }

    fn commit(mut self, target: &CString) -> io::Result<()> {
        // `renameat` replaces the target directory entry itself. It follows
        // neither a symlink nor a hardlink, even if one appears after the
        // compatibility check in `refuse_existing_symlink`.
        // SAFETY: both names are NUL-terminated and both directory descriptors
        // refer to the same live parent.
        let renamed = unsafe {
            libc::renameat(
                self.parent.as_raw_fd(),
                self.name.as_ptr(),
                self.parent.as_raw_fd(),
                target.as_ptr(),
            )
        };
        if renamed == -1 {
            return Err(io::Error::last_os_error());
        }
        self.committed = true;
        Ok(())
    }
}

#[cfg(unix)]
impl Drop for StagedFile<'_> {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        // Best-effort cleanup after any write or rename error. The name was
        // created with `O_EXCL` beneath this exact held directory descriptor.
        // SAFETY: `name` and `parent` remain live for the duration of `drop`.
        unsafe {
            libc::unlinkat(self.parent.as_raw_fd(), self.name.as_ptr(), 0);
        }
    }
}

#[cfg(unix)]
fn refuse_existing_symlink(parent: &OwnedFd, name: &CString) -> io::Result<()> {
    let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
    // This check preserves the existing fail-closed behavior for a stable final
    // symlink. Safety does not depend on the check: the later `renameat`
    // replaces whatever directory entry exists at that instant without
    // following it, so a concurrent swap cannot redirect the write.
    // SAFETY: `name` is NUL-terminated, `parent` is live, and `metadata` points
    // to enough writable storage for `fstatat`.
    let result = unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            name.as_ptr(),
            metadata.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result == -1 {
        let error = io::Error::last_os_error();
        return if error.kind() == io::ErrorKind::NotFound {
            Ok(())
        } else {
            Err(error)
        };
    }
    // SAFETY: successful `fstatat` initialized the structure above.
    let metadata = unsafe { metadata.assume_init() };
    if metadata.st_mode & libc::S_IFMT == libc::S_IFLNK {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a runtime prelude target may not be a symlink",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn component_name(name: &OsStr) -> io::Result<CString> {
    CString::new(name.as_bytes()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "a runtime prelude path component contains a null byte",
        )
    })
}

#[cfg(unix)]
fn owned_descriptor(descriptor: libc::c_int) -> io::Result<OwnedFd> {
    if descriptor == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful `open`/`openat` calls return a new descriptor whose
    // ownership has not been transferred anywhere else.
    Ok(unsafe { OwnedFd::from_raw_fd(descriptor) })
}

#[cfg(not(unix))]
struct AnchoredHome;

#[cfg(not(unix))]
impl AnchoredHome {
    fn open(_home: &Path) -> io::Result<Self> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "the runtime prelude writer requires directory-relative file operations",
        ))
    }

    fn write(&self, _relative: &Path, _body: &[u8]) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "the runtime prelude writer requires directory-relative file operations",
        ))
    }
}

/// A relative path that cannot leave the directory it is joined to.
///
/// The value arrives as environment set by a control plane, which is trusted —
/// but a path is the one kind of value where trusting is cheap to avoid, and
/// where being wrong writes a file somewhere this process should not.
fn contained(raw: &str) -> Option<PathBuf> {
    let path = Path::new(raw);
    if path.is_absolute() {
        return None;
    }
    let mut cleaned = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) if part.len() <= PACK_COMPONENT_LIMIT => cleaned.push(part),
            Component::Normal(_) => return None,
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    match cleaned.as_os_str().is_empty() {
        true => None,
        false => Some(cleaned),
    }
}

#[cfg(test)]
#[path = "prelude_tests.rs"]
mod prelude_tests;
