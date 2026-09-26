//! The processes of this user that run a given executable file, read from
//! the platform's process table: `proc_listallpids`, `proc_pidinfo` and
//! `proc_pidpath` on macOS, `/proc/<pid>` on Linux. `update` uses it to
//! refuse replacing a binary other processes still run (`ARCHITECTURE.md`
//! §What runs where).
//!
//! Each listed process is gone (exited, or a zombie), identified, or live
//! but unidentified. An identified process runs the file when the path the
//! kernel reports for its executable is the file's canonical path, or when
//! its image is the file's inode. The path catches a process still running
//! an inode an earlier upgrade renamed over, since the kernel keeps
//! reporting the path it was started from; the inode catches a process that
//! reached the file through another path, such as a hard link.
//!
//! A process whose executable cannot be identified is counted when its
//! process name is the file's name, or when not even its name can be read.
//! Refusing every unidentified process would block on `ssh-agent` and
//! `gpg-agent`, which Linux hides from their own user; ignoring them missed
//! a live process running an old release whose last link was removed. A
//! renamed copy of the binary whose image cannot be read is therefore
//! missed.
//!
//! Nothing here returns argument or environment text read from another
//! process beyond an exact subcommand name and `COMMONMEASURE_HOME`: other
//! processes' arguments and environments can carry credentials.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// A live process of this user that runs the file, or may.
#[derive(Debug, PartialEq)]
pub struct Process {
    pub pid: u32,
    pub runs: Runs,
    /// The first argument after the program name that is exactly one of
    /// the subcommand names `running` was given.
    pub subcommand: Option<String>,
    pub home: Home,
    /// It runs the file at the pid launchd first reported for the managed
    /// console, as launchd's child, but started after launchd was asked, or
    /// launchd reported another pid or none when asked again: the console
    /// service restarted or stopped during the check.
    pub console_changed: bool,
}

/// How a process was matched to the file.
#[derive(Debug, PartialEq)]
pub enum Runs {
    /// Its executable is the file. `path` is the executable's path as the
    /// kernel reports it, when it reports one.
    File { path: Option<PathBuf> },
    /// Its executable could not be identified, and its process name is the
    /// file's name, or could not be read either (`None`). On macOS the name
    /// is unreadable only for a process the kernel refused to describe
    /// whose owner could not be read either: nothing shows it runs the file,
    /// and nothing rules it out.
    Unidentified { name: Option<String> },
}

/// The Edge home a process was started with.
#[derive(Debug, PartialEq)]
pub enum Home {
    /// `COMMONMEASURE_HOME` was set to this.
    Set(String),
    /// Its environment was read, holds at least one variable, and does not
    /// set `COMMONMEASURE_HOME`, or sets it empty.
    Default,
    /// Its environment could not be read, was read empty, or holds an empty
    /// entry followed by more entries, none of them `COMMONMEASURE_HOME`; or,
    /// on macOS, the variable was found only past where the kernel's own
    /// strings were taken to start. macOS returns a restricted process's
    /// arguments without its environment, so an empty read does not show
    /// that the variable is unset; and an empty entry, which `execve`
    /// accepts, leaves open whether what follows it is still the
    /// environment.
    Unknown,
}

/// The managed console as launchd reported it, which `running` leaves out
/// only while it is still that process.
pub struct Console<'a> {
    pub pid: u32,
    /// When launchd was asked for `pid`. The process it reported had
    /// started by then; one started later reuses the pid.
    pub asked: SystemTime,
    /// Asks launchd again, after the scan, for the pid it reports now.
    pub again: &'a dyn Fn() -> Option<u32>,
}

impl Console<'_> {
    /// Whether the process at `pid`, which runs the file, may be the one
    /// launchd reported: the same pid, a child of launchd, and started
    /// before launchd was asked. `running` still asks launchd again before
    /// leaving it out.
    fn admits(&self, pid: u32, mine: &Mine) -> bool {
        pid == self.pid
            && mine.parent == 1
            && mine.started.is_some_and(|started| started < self.asked)
    }
}

/// The processes of the current user, other than this one, that run
/// `file`. `console` is the managed console: the process at its pid is left
/// out only when it runs `file`, its parent is launchd (pid 1), it started
/// before launchd was asked, and launchd reports the same pid again after
/// the scan. The start time rules out a process that took the pid after
/// launchd answered, such as an orphaned MCP server, whose parent is also
/// launchd; the second answer rules out a console launchd stopped reporting
/// during the scan. Start times are wall-clock, so a clock set back between
/// the two can defeat the first check. Without a start time, as on Linux,
/// the console is counted like any other process. An error means the table
/// could not be read in full, which is not the same as finding no process.
pub fn running(
    file: &Path,
    console: Option<&Console>,
    subcommands: &[String],
) -> Result<Vec<Process>, String> {
    let target = Target {
        canonical: file
            .canonicalize()
            .map_err(|error| format!("{}: {error}", file.display()))?,
        image: identity(file).ok_or_else(|| format!("{}: cannot stat", file.display()))?,
        name: file
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| format!("{}: no file name", file.display()))?
            .to_string(),
    };
    let me = std::process::id();
    let mut found = Vec::new();
    let mut console_process = None;
    for pid in table::pids()? {
        if pid == me {
            continue;
        }
        let Entry::Mine(mine) = table::entry(pid)? else {
            continue;
        };
        let identity = mine.executable.identify(&target);
        let at_console = identity == Identity::Target
            && console.is_some_and(|console| pid == console.pid && mine.parent == 1);
        let admitted = at_console && console.is_some_and(|console| console.admits(pid, &mine));
        let runs = match identity {
            Identity::Target => Runs::File {
                path: mine.executable.path,
            },
            Identity::Other => continue,
            Identity::Unknown => match unidentified(mine.name, &target.name, mine.name_limit) {
                Some(runs) => runs,
                None => continue,
            },
        };
        let (subcommand, home) = table::shown(pid, subcommands);
        let process = Process {
            pid,
            runs,
            subcommand,
            home,
            console_changed: at_console && !admitted,
        };
        if admitted {
            console_process = Some(process);
        } else {
            found.push(process);
        }
    }
    if let (Some(process), Some(console)) = (console_process, console)
        && (console.again)() != Some(process.pid)
    {
        found.push(Process {
            console_changed: true,
            ..process
        });
    }
    found.sort_by_key(|process| process.pid);
    Ok(found)
}

/// The file being looked for.
struct Target {
    canonical: PathBuf,
    image: (u64, u64),
    name: String,
}

/// One pid as the process table describes it.
enum Entry {
    /// Exited, or a zombie, since it was listed.
    Gone,
    /// Another user's, or one the kernel does not describe to this user.
    Other,
    Mine(Mine),
}

struct Mine {
    parent: u32,
    /// When it started, where the kernel says.
    started: Option<SystemTime>,
    /// The process name: the executable's file name when it started,
    /// truncated to `name_limit` bytes. `None` when it could not be read.
    name: Option<String>,
    name_limit: usize,
    executable: Executable,
}

/// What the kernel says of a process's executable.
#[derive(Debug, Default)]
struct Executable {
    /// The path the kernel reports for it.
    path: Option<PathBuf>,
    /// `path` resolved, when it still resolves.
    canonical: Option<PathBuf>,
    /// The files it runs: on Linux its image; on macOS the file at `path`,
    /// or with no path, each file it maps executable.
    images: Vec<Image>,
}

#[derive(Debug, Clone, Copy)]
struct Image {
    /// Device and inode.
    id: (u64, u64),
    /// Whether the file still has a name on disk: false once its last link
    /// is removed.
    linked: bool,
}

#[derive(Debug, PartialEq)]
enum Identity {
    Target,
    Other,
    Unknown,
}

impl Executable {
    fn identify(&self, target: &Target) -> Identity {
        if self.canonical.as_deref() == Some(target.canonical.as_path())
            || self.images.iter().any(|image| image.id == target.image)
        {
            Identity::Target
        } else if self.path.is_some()
            || (!self.images.is_empty() && self.images.iter().all(|image| image.linked))
        {
            Identity::Other
        } else {
            // No path, and an image with no name left on disk, or none read:
            // it may be an old release started from the file's path.
            Identity::Unknown
        }
    }
}

/// Whether a process whose executable is not identified is counted: when
/// its name, truncated to `limit` bytes, is the file's name, or when its
/// name could not be read either, since then nothing rules it out.
fn unidentified(name: Option<String>, file_name: &str, limit: usize) -> Option<Runs> {
    match name {
        Some(name) if same_name(&name, file_name, limit) => {
            Some(Runs::Unidentified { name: Some(name) })
        }
        Some(_) => None,
        None => Some(Runs::Unidentified { name: None }),
    }
}

/// Whether process name `name`, which the kernel truncates to `limit`
/// bytes, is the name of file `file_name`.
fn same_name(name: &str, file_name: &str, limit: usize) -> bool {
    !name.is_empty() && (name == file_name || (name.len() == limit && file_name.starts_with(name)))
}

/// What `update` may show of a process's arguments and environment: the
/// first argument after the program name that is one of `subcommands`, and
/// the home [`home`] reads from the environment, unknown when it was not
/// read.
fn shown<'a>(
    arguments: impl IntoIterator<Item = &'a [u8]>,
    environment: Option<impl IntoIterator<Item = &'a [u8]>>,
    subcommands: &[String],
) -> (Option<String>, Home) {
    let subcommand = arguments
        .into_iter()
        .skip(1)
        .find_map(|argument| subcommands.iter().find(|name| name.as_bytes() == argument))
        .cloned();
    (subcommand, environment.map_or(Home::Unknown, home))
}

/// `COMMONMEASURE_HOME` from the entries of an environment as read, empty
/// entries included. The variable found anywhere among them is the home.
/// Otherwise the home is the default only when at least one entry was read
/// and no empty entry is followed by another: nothing read is what macOS
/// returns for a restricted process, and past an empty entry the read has
/// not shown where the environment ends.
fn home<'a>(environment: impl IntoIterator<Item = &'a [u8]>) -> Home {
    let (mut read, mut empty, mut resumed) = (false, false, false);
    for entry in environment {
        if let Some(home) = entry.strip_prefix(HOME_KEY) {
            let home = String::from_utf8_lossy(home);
            return match home.trim() {
                "" => Home::Default,
                _ => Home::Set(home.into_owned()),
            };
        }
        match (entry.is_empty(), empty) {
            (true, _) => empty = true,
            (false, false) => read = true,
            (false, true) => resumed = true,
        }
    }
    if read && !resumed {
        Home::Default
    } else {
        Home::Unknown
    }
}

/// How an environment entry that sets the Edge home starts.
const HOME_KEY: &[u8] = b"COMMONMEASURE_HOME=";

/// The NUL-terminated strings of a `/proc/<pid>/cmdline` or `environ` file.
/// Zero bytes read as one empty entry, which [`home`] does not take as an
/// environment that was read.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn proc_strings(bytes: &[u8]) -> Vec<&[u8]> {
    bytes
        .strip_suffix(b"\0")
        .unwrap_or(bytes)
        .split(|&byte| byte == 0)
        .collect()
}

#[cfg(unix)]
fn identity(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt as _;
    let metadata = std::fs::metadata(path).ok()?;
    Some((metadata.dev(), metadata.ino()))
}

#[cfg(not(unix))]
fn identity(_: &Path) -> Option<(u64, u64)> {
    None
}

/// The pids `list` reports: called once without a buffer for the count, and
/// once with room for that many and more. A second answer that fills the
/// buffer may have been cut short, so it is an error, not a short list.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn listed(mut list: impl FnMut(Option<&mut [i32]>) -> i32) -> Result<Vec<u32>, String> {
    let count = list(None);
    if count <= 0 {
        return Err(format!(
            "list processes: {}",
            std::io::Error::last_os_error()
        ));
    }
    // Room for processes started between the two calls.
    let mut pids = vec![0; usize::try_from(count).unwrap_or(0) + 256];
    let filled = list(Some(&mut pids));
    let filled = usize::try_from(filled)
        .ok()
        .filter(|&filled| filled > 0)
        .ok_or_else(|| format!("list processes: {}", std::io::Error::last_os_error()))?;
    if filled >= pids.len() {
        return Err(format!(
            "list processes: more than {} processes, and more were starting while they were \
             listed",
            pids.len()
        ));
    }
    pids.truncate(filled);
    Ok(pids
        .into_iter()
        .filter_map(|pid| u32::try_from(pid).ok())
        .filter(|&pid| pid != 0)
        .collect())
}

/// One mapped region, as `mapped` walks them.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
struct Region {
    /// The address after it, `None` when that overflows.
    end: Option<u64>,
    /// The file it maps, when it maps a regular file executable.
    image: Option<Image>,
}

/// Every executable file a process maps, from `region`, which gives the
/// region at or after an address, or `None` past the last. The walk does not
/// stop at the first image: the main executable was first in every region
/// table seen, but nothing guarantees that, and a library mapped before it
/// would otherwise hide it.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn mapped<E>(mut region: impl FnMut(u64) -> Result<Option<Region>, E>) -> Result<Vec<Image>, E> {
    let mut images = Vec::new();
    let mut address = 0u64;
    while let Some(Region { end, image }) = region(address)? {
        images.extend(image);
        match end {
            Some(next) if next > address => address = next,
            _ => break,
        }
    }
    Ok(images)
}

/// A process's owner as read independently of the call that refused to
/// describe it.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
enum Owner {
    Gone,
    Read { uid: u32, parent: u32, name: String },
    Unreadable,
}

/// A process the kernel refused to describe (EPERM), by its owner. A
/// security policy can refuse a process of this user too, so EPERM alone
/// does not make it another user's: the same user's is unidentified, and
/// the name rule applies; one whose owner cannot be read is unidentified
/// with no name, so it is counted.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn denied(owner: Owner, me: u32) -> Entry {
    match owner {
        Owner::Gone => Entry::Gone,
        Owner::Read { uid, .. } if uid != me => Entry::Other,
        Owner::Read { parent, name, .. } => Entry::Mine(Mine {
            parent,
            started: None,
            name: Some(name),
            // `p_comm` holds this many bytes and a NUL.
            name_limit: MAXCOMLEN,
            executable: Executable::default(),
        }),
        Owner::Unreadable => Entry::Mine(Mine {
            parent: 0,
            started: None,
            name: None,
            name_limit: 0,
            executable: Executable::default(),
        }),
    }
}

/// `MAXCOMLEN` from `<sys/param.h>`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const MAXCOMLEN: usize = 16;

/// NUL-terminated strings, borrowed from the buffer they were read into.
type Strings<'a> = Vec<&'a [u8]>;

/// Reads `KERN_PROCARGS2`: the argument count as a native `int`, the
/// executable path, NUL padding, then the arguments and the environment as
/// NUL-terminated strings, then the strings the kernel passes the process
/// beside its environment (`apple[]` in XNU). Returns the arguments and the
/// environment, its empty entries kept and the NULs after it dropped, or no
/// environment when where it ends is in doubt (below).
///
/// The padding runs to the next multiple of eight bytes after the path's
/// NUL: XNU pads the strings area to pointer size after an
/// `executable_path=` prefix of 16 bytes, which the sysctl strips. The
/// offset is what tells an empty argument zero from the padding. It is the
/// kernel's layout, not an interface, so a buffer whose padding is not all
/// NUL, or that holds fewer arguments than its count, reads as `None`.
///
/// The environment ends before the first string that follows an empty one
/// and names one of the kernel's own keys ([`APPLE`]). An environment entry
/// may also be named like one of those keys, so that end can fall early.
/// None of the kernel's keys is `COMMONMEASURE_HOME`: when an entry that
/// sets it comes after that end and none before it, the end may be wrong,
/// and no environment is returned. When no entry anywhere after the
/// arguments sets it, the variable is unset wherever the end falls. When no
/// kernel string is found, as when a later macOS adds a key, the kernel's
/// strings stay in the environment after an empty entry, and [`home`] reads
/// the home as unknown unless the variable is among them.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn procargs(buffer: &[u8]) -> Option<(Strings<'_>, Option<Strings<'_>>)> {
    let count = usize::try_from(i32::from_ne_bytes(buffer.get(..4)?.try_into().ok()?)).ok()?;
    let rest = buffer.get(4..)?;
    let path_end = rest.iter().position(|&byte| byte == 0)?;
    let start = (path_end + 1).next_multiple_of(8);
    if !rest.get(path_end..start)?.iter().all(|&byte| byte == 0) {
        return None;
    }
    let mut strings = rest[start..].split(|&byte| byte == 0);
    let arguments: Vec<&[u8]> = strings.by_ref().take(count).collect();
    if arguments.len() != count {
        return None;
    }
    let mut environment: Vec<&[u8]> = strings.collect();
    let apple = |entry: &[u8]| {
        APPLE.iter().any(|key| {
            entry
                .strip_prefix(key.as_bytes())
                .is_some_and(|value| value.starts_with(b"="))
        })
    };
    if let Some(end) = environment
        .windows(2)
        .position(|pair| pair[0].is_empty() && apple(pair[1]))
    {
        let sets = |entry: &&[u8]| entry.starts_with(HOME_KEY);
        if !environment[..end].iter().any(sets) && environment[end..].iter().any(sets) {
            return Some((arguments, None));
        }
        environment.truncate(end);
    }
    while environment.last().is_some_and(|entry| entry.is_empty()) {
        environment.pop();
    }
    Some((arguments, Some(environment)))
}

/// Keys of the strings XNU passes a process after its environment
/// (`exec_add_apple_strings`), as `key=value`, as read on macOS 26.4. The
/// area is read from the process's own memory, which the process changes
/// once it runs: first `pfz=`, `stack_guard=` and `malloc_entropy=` follow a
/// few NULs; once the C library has cleared those, `ptr_munge=` follows a
/// longer run.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const APPLE: [&str; 12] = [
    "pfz",
    "stack_guard",
    "malloc_entropy",
    "ptr_munge",
    "main_stack",
    "executable_file",
    "dyld_file",
    "executable_cdhash",
    "executable_boothash",
    "arm64e_abi",
    "th_port",
    "security_config",
];

#[cfg(target_os = "macos")]
mod table {
    use super::{Entry, Executable, Home, Image, Mine};
    use std::path::PathBuf;

    /// `pbi_name` holds this many bytes of the process name and a NUL.
    const NAME_LIMIT: usize = 2 * libc::MAXCOMLEN - 1;

    /// `PROC_PIDREGIONPATHINFO` in `<sys/proc_info.h>`: the mapped region
    /// at or after an address, with its file's vnode when it has one.
    const PROC_PIDREGIONPATHINFO: libc::c_int = 8;
    const VREG: libc::c_int = 1;

    /// `struct proc_regionwithpathinfo`.
    #[repr(C)]
    struct RegionWithPath {
        protection: u32,
        _before_address: [u32; 3],
        _offset: u64,
        _counters: [u32; 14],
        address: u64,
        size: u64,
        vnode: libc::vnode_info_path,
    }
    const _: () = assert!(size_of::<RegionWithPath>() == 1272);

    pub fn pids() -> Result<Vec<u32>, String> {
        super::listed(|buffer| match buffer {
            // SAFETY: a null buffer asks only for the count.
            None => unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) },
            Some(buffer) => {
                let Ok(bytes) = libc::c_int::try_from(size_of_val(buffer)) else {
                    return -1;
                };
                // SAFETY: the buffer holds `bytes` bytes of c_int.
                unsafe { libc::proc_listallpids(buffer.as_mut_ptr().cast(), bytes) }
            }
        })
    }

    fn errno() -> Option<i32> {
        std::io::Error::last_os_error().raw_os_error()
    }

    pub fn entry(pid: u32) -> Result<Entry, String> {
        let Ok(pid) = libc::c_int::try_from(pid) else {
            return Ok(Entry::Other);
        };
        // SAFETY: zeroed is a valid proc_bsdinfo, and proc_pidinfo writes at
        // most `size` bytes into it.
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = size_of::<libc::proc_bsdinfo>() as libc::c_int;
        let written = unsafe {
            libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size)
        };
        if written != size {
            return match errno() {
                // Exited, or a zombie: the kernel describes neither.
                Some(libc::ESRCH) => Ok(Entry::Gone),
                // Another user's process, or one a security policy withholds
                // from its own user: the owner decides which.
                Some(libc::EPERM) => Ok(super::denied(owner(pid), me())),
                _ => Err(format!(
                    "read process {pid}: {}",
                    std::io::Error::last_os_error()
                )),
            };
        }
        if info.pbi_status == libc::SZOMB {
            return Ok(Entry::Gone);
        }
        if info.pbi_uid != me() {
            return Ok(Entry::Other);
        }
        let name = match text(&info.pbi_name) {
            name if name.is_empty() => text(&info.pbi_comm),
            name => name,
        };
        let started = std::time::UNIX_EPOCH.checked_add(
            std::time::Duration::from_secs(info.pbi_start_tvsec)
                + std::time::Duration::from_micros(info.pbi_start_tvusec),
        );
        let executable = match path(pid) {
            Ok(path) => Executable {
                canonical: path.canonicalize().ok(),
                images: super::identity(&path)
                    .map(|id| Image { id, linked: true })
                    .into_iter()
                    .collect(),
                path: Some(path),
            },
            Err(Some(libc::ESRCH)) => return Ok(Entry::Gone),
            Err(_) => match images(pid) {
                Ok(images) => Executable {
                    images,
                    ..Executable::default()
                },
                Err(Some(libc::ESRCH)) => return Ok(Entry::Gone),
                Err(_) => Executable::default(),
            },
        };
        Ok(Entry::Mine(Mine {
            parent: info.pbi_ppid,
            started,
            name: Some(name),
            name_limit: NAME_LIMIT,
            executable,
        }))
    }

    fn me() -> u32 {
        // SAFETY: getuid cannot fail.
        unsafe { libc::getuid() }
    }

    /// `struct kinfo_proc` from `<sys/sysctl.h>`, read as bytes at the
    /// offsets the header gives on 64-bit macOS; libc does not declare it.
    const KINFO_SIZE: usize = 648;
    const KINFO_STAT: usize = 36;
    const KINFO_PID: usize = 40;
    const KINFO_COMM: usize = 243;
    const KINFO_UID: usize = 420;
    const KINFO_PARENT: usize = 560;

    /// The owner, parent and name of `pid` from `sysctl` `KERN_PROC_PID`,
    /// which describes any process to any user: the kernel's check that
    /// refuses `proc_pidinfo` does not apply to it.
    pub(super) fn owner(pid: libc::c_int) -> super::Owner {
        let mut buffer = [0u8; KINFO_SIZE];
        let mut size = buffer.len();
        let mut mib = [libc::CTL_KERN, libc::KERN_PROC, libc::KERN_PROC_PID, pid];
        // SAFETY: the buffer holds `size` bytes, and sysctl writes no more.
        let status = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                4,
                buffer.as_mut_ptr().cast(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        let int = |at: usize| i32::from_ne_bytes(buffer[at..at + 4].try_into().unwrap());
        match (status, size) {
            // An absent pid reads as no bytes.
            (0, 0) => super::Owner::Gone,
            // The pid field doubles as a check that the layout still holds.
            (0, KINFO_SIZE) if int(KINFO_PID) == pid => {
                if buffer[KINFO_STAT] == libc::SZOMB as u8 {
                    return super::Owner::Gone;
                }
                let comm = &buffer[KINFO_COMM..KINFO_COMM + libc::MAXCOMLEN + 1];
                let name: Vec<u8> = comm.iter().copied().take_while(|&byte| byte != 0).collect();
                super::Owner::Read {
                    uid: int(KINFO_UID) as u32,
                    parent: int(KINFO_PARENT) as u32,
                    name: String::from_utf8_lossy(&name).into_owned(),
                }
            }
            _ => super::Owner::Unreadable,
        }
    }

    fn text(bytes: &[libc::c_char]) -> String {
        let bytes: Vec<u8> = bytes
            .iter()
            .map(|&byte| byte as u8)
            .take_while(|&byte| byte != 0)
            .collect();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    /// `proc_pidpath`. It fails with ENOENT once the image's last link is
    /// removed, though the process runs on.
    fn path(pid: libc::c_int) -> Result<PathBuf, Option<i32>> {
        let mut buffer = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
        // SAFETY: the buffer holds the size passed.
        let length =
            unsafe { libc::proc_pidpath(pid, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
        let length = usize::try_from(length)
            .ok()
            .filter(|&length| length > 0)
            .ok_or_else(errno)?;
        buffer.truncate(length);
        use std::os::unix::ffi::OsStringExt as _;
        Ok(PathBuf::from(std::ffi::OsString::from_vec(buffer)))
    }

    /// With no path to go on: the files the process maps executable, from
    /// its region table. They include its own image, which keeps its inode
    /// after its last link is removed.
    fn images(pid: libc::c_int) -> Result<Vec<Image>, Option<i32>> {
        super::mapped(|address| {
            // SAFETY: zeroed is a valid RegionWithPath, and proc_pidinfo
            // writes at most `size` bytes into it.
            let mut region: RegionWithPath = unsafe { std::mem::zeroed() };
            let size = size_of::<RegionWithPath>() as libc::c_int;
            let written = unsafe {
                libc::proc_pidinfo(
                    pid,
                    PROC_PIDREGIONPATHINFO,
                    address,
                    (&raw mut region).cast(),
                    size,
                )
            };
            if written != size {
                // EINVAL past the last region.
                return match errno() {
                    Some(libc::EINVAL) if address > 0 => Ok(None),
                    error => Err(error),
                };
            }
            let vnode = &region.vnode.vip_vi;
            let executable_file =
                vnode.vi_type == VREG && region.protection & libc::VM_PROT_EXECUTE as u32 != 0;
            Ok(Some(super::Region {
                end: region.address.checked_add(region.size),
                image: executable_file.then(|| Image {
                    id: (u64::from(vnode.vi_stat.vst_dev), vnode.vi_stat.vst_ino),
                    linked: vnode.vi_stat.vst_nlink > 0,
                }),
            }))
        })
    }

    /// The subcommand and home from `KERN_PROCARGS2`.
    pub fn shown(pid: u32, subcommands: &[String]) -> (Option<String>, Home) {
        match procargs2(pid).as_deref().and_then(super::procargs) {
            Some((arguments, environment)) => super::shown(arguments, environment, subcommands),
            None => (None, Home::Unknown),
        }
    }

    fn procargs2(pid: u32) -> Option<Vec<u8>> {
        let pid = libc::c_int::try_from(pid).ok()?;
        let mut argmax: libc::c_int = 0;
        let mut size = size_of::<libc::c_int>();
        let mut mib = [libc::CTL_KERN, libc::KERN_ARGMAX];
        // SAFETY: mib names an int-valued sysctl and size is its size.
        let status = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                2,
                (&raw mut argmax).cast(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        if status != 0 || argmax <= 0 {
            return None;
        }
        let mut buffer = vec![0u8; usize::try_from(argmax).ok()?];
        let mut size = buffer.len();
        let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
        // SAFETY: the buffer holds `size` bytes, and sysctl writes no more.
        let status = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                3,
                buffer.as_mut_ptr().cast(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        (status == 0).then(|| {
            buffer.truncate(size);
            buffer
        })
    }
}

#[cfg(target_os = "linux")]
mod table {
    use super::{Entry, Executable, Home, Image, Mine};
    use std::path::PathBuf;

    /// `/proc/<pid>/comm` holds this many bytes of the process name.
    const NAME_LIMIT: usize = 15;

    /// Every numeric entry of `/proc`. An error reading the directory is an
    /// error for the whole list, not a skipped entry.
    pub fn pids() -> Result<Vec<u32>, String> {
        let mut pids = Vec::new();
        for entry in std::fs::read_dir("/proc").map_err(|error| format!("read /proc: {error}"))? {
            let entry = entry.map_err(|error| format!("read /proc: {error}"))?;
            if let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse().ok())
            {
                pids.push(pid);
            }
        }
        Ok(pids)
    }

    /// Whether a read under `/proc/<pid>` failed because the process has
    /// gone.
    fn gone(error: &std::io::Error) -> bool {
        error.kind() == std::io::ErrorKind::NotFound || error.raw_os_error() == Some(libc::ESRCH)
    }

    pub fn entry(pid: u32) -> Result<Entry, String> {
        let read = |name: &str| std::fs::read(format!("/proc/{pid}/{name}"));
        // `status` is readable for every process, including one that is not
        // dumpable, whose other files belong to root.
        let status = match read("status") {
            Ok(status) => String::from_utf8_lossy(&status).into_owned(),
            Err(error) if gone(&error) => return Ok(Entry::Gone),
            Err(error) => return Err(format!("read /proc/{pid}/status: {error}")),
        };
        let field = |name: &str| {
            status
                .lines()
                .find_map(|line| line.strip_prefix(name))
                .map(str::trim)
        };
        if matches!(
            field("State:").and_then(|state| state.chars().next()),
            Some('Z' | 'X')
        ) {
            return Ok(Entry::Gone);
        }
        // Real, effective, saved and filesystem uids: the effective one.
        let uid =
            field("Uid:").and_then(|uids| uids.split_whitespace().nth(1)?.parse::<u32>().ok());
        let parent = field("PPid:").and_then(|parent| parent.parse::<u32>().ok());
        let (Some(uid), Some(parent)) = (uid, parent) else {
            return Err(format!("read /proc/{pid}/status: no Uid or PPid line"));
        };
        // SAFETY: getuid cannot fail.
        if uid != unsafe { libc::getuid() } {
            return Ok(Entry::Other);
        }
        let name = match read("comm") {
            Ok(name) => String::from_utf8_lossy(&name)
                .trim_end_matches('\n')
                .to_string(),
            Err(error) if gone(&error) => return Ok(Entry::Gone),
            Err(error) => return Err(format!("read /proc/{pid}/comm: {error}")),
        };
        let link = format!("/proc/{pid}/exe");
        let executable = match std::fs::read_link(&link) {
            Ok(target) => {
                // An executable renamed over or removed since it started
                // reads back with this suffix; the path it was started from
                // is what matches.
                let text = target.to_string_lossy();
                let (path, linked) = match text.strip_suffix(" (deleted)") {
                    Some(path) => (PathBuf::from(path), false),
                    None => (target.clone(), true),
                };
                // Through the link, which resolves to the image the process
                // runs even after its path was renamed over.
                use std::os::unix::fs::MetadataExt as _;
                let images = match std::fs::metadata(&link) {
                    Ok(metadata) => vec![Image {
                        id: (metadata.dev(), metadata.ino()),
                        linked,
                    }],
                    Err(error) if gone(&error) => return Ok(Entry::Gone),
                    Err(_) => Vec::new(),
                };
                Executable {
                    canonical: path.canonicalize().ok(),
                    path: Some(path),
                    images,
                }
            }
            Err(error) if gone(&error) => return Ok(Entry::Gone),
            // EACCES: the process is not dumpable (`PR_SET_DUMPABLE`), and
            // no file of `/proc` names its image to its own user: `exe`,
            // `maps` and `map_files` are all withheld.
            Err(_) => Executable::default(),
        };
        // Service mode, and with it the console exemption, is macOS only.
        Ok(Entry::Mine(Mine {
            parent,
            started: None,
            name: Some(name),
            name_limit: NAME_LIMIT,
            executable,
        }))
    }

    /// The subcommand from `cmdline` and the home from `environ`, which a
    /// process that is not dumpable withholds.
    pub fn shown(pid: u32, subcommands: &[String]) -> (Option<String>, Home) {
        let read = |name: &str| std::fs::read(format!("/proc/{pid}/{name}")).ok();
        let arguments = read("cmdline").unwrap_or_default();
        let environment = read("environ");
        super::shown(
            super::proc_strings(&arguments),
            environment.as_deref().map(super::proc_strings),
            subcommands,
        )
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod table {
    use super::{Entry, Home};

    pub fn pids() -> Result<Vec<u32>, String> {
        Err("listing the processes that run a binary is supported on macOS and Linux".into())
    }
    pub fn entry(_: u32) -> Result<Entry, String> {
        Ok(Entry::Other)
    }
    pub fn shown(_: u32, _: &[String]) -> (Option<String>, Home) {
        (None, Home::Unknown)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SENTINEL: &str = "AAA_SENTINEL=NOT_A_CREDENTIAL";

    /// A `KERN_PROCARGS2` buffer as XNU lays it out: padding after the
    /// path's NUL to a multiple of eight bytes.
    fn procargs2(path: &str, arguments: &[&str], environment: &[&str]) -> Vec<u8> {
        let mut buffer = i32::try_from(arguments.len())
            .unwrap()
            .to_ne_bytes()
            .to_vec();
        buffer.extend_from_slice(path.as_bytes());
        buffer.resize(4 + (path.len() + 1).next_multiple_of(8), 0);
        for string in arguments.iter().chain(environment) {
            buffer.extend_from_slice(string.as_bytes());
            buffer.push(0);
        }
        buffer.extend_from_slice(&[0; 3]);
        buffer
    }

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn procargs_reads_the_arguments_and_the_environment() {
        let buffer = procargs2(
            "/bin/commonmeasure",
            &["/bin/commonmeasure", "mcp", "--host=codex"],
            &["HOME=/home/op", "COMMONMEASURE_HOME=/edge"],
        );
        let (arguments, environment) = procargs(&buffer).unwrap();
        assert_eq!(
            arguments,
            [&b"/bin/commonmeasure"[..], b"mcp", b"--host=codex"]
        );
        assert_eq!(
            environment.unwrap(),
            [&b"HOME=/home/op"[..], b"COMMONMEASURE_HOME=/edge"]
        );

        // Padding that is not all NUL, or fewer arguments than counted, is
        // not a layout this reads.
        let mut shifted = buffer.clone();
        shifted[4 + b"/bin/commonmeasure".len() + 2] = b'x';
        assert_eq!(procargs(&shifted), None);
        let short = [&3i32.to_ne_bytes()[..], b"/bin/x\0\0/bin/x\0"].concat();
        assert_eq!(procargs(&short), None);
        assert_eq!(procargs(&[1, 0]), None);
    }

    #[test]
    fn an_empty_argument_zero_does_not_move_the_environment_into_the_arguments() {
        // The reviewer's case: argv ["", "mcp", "--host", "claude-code"], a
        // credential-shaped entry first in the environment. For every path
        // length, so the padding takes each of its eight sizes.
        for length in 1..=16 {
            let path = format!("/{}", "p".repeat(length));
            let buffer = procargs2(
                &path,
                &["", "mcp", "--host", "claude-code"],
                &[SENTINEL, "COMMONMEASURE_HOME=/edge/a"],
            );
            let (arguments, environment) = procargs(&buffer).unwrap();
            assert_eq!(arguments, [&b""[..], b"mcp", b"--host", b"claude-code"]);
            assert_eq!(environment.as_ref().unwrap()[0], SENTINEL.as_bytes());
            let shown = shown(arguments, environment, &names(&["mcp", "serve"]));
            assert_eq!(
                shown,
                (Some("mcp".into()), Home::Set("/edge/a".into())),
                "{path}"
            );
        }
    }

    /// A `KERN_PROCARGS2` buffer as macOS 26.4 returns it for a process
    /// started with `environment`: the arguments, the environment, NULs,
    /// then the kernel's own strings, in the form read before the process
    /// clears some of them (`cleared` false) or after.
    fn procargs2_with_apple(arguments: &[&str], environment: &[&str], cleared: bool) -> Vec<u8> {
        let mut buffer = procargs2("/bin/commonmeasure", arguments, environment);
        buffer.truncate(buffer.len() - 3);
        let apple: &[&str] = if cleared {
            &[
                "",
                "",
                "ptr_munge=",
                "main_stack=",
                "executable_file=0x1a0100000d,0x3c4f2",
            ]
        } else {
            &[
                "",
                "pfz=0x7ffe00000",
                "stack_guard=0x9c1",
                "ptr_munge=0x55",
                "th_port=0x103",
            ]
        };
        for string in apple {
            buffer.extend_from_slice(string.as_bytes());
            buffer.push(0);
        }
        buffer
    }

    #[test]
    fn the_macos_environment_ends_at_the_kernel_s_strings_and_an_empty_entry_is_not_its_end() {
        for cleared in [false, true] {
            let home = |environment: &[&str]| {
                let buffer =
                    procargs2_with_apple(&["/bin/commonmeasure", "mcp"], environment, cleared);
                let (arguments, environment) = procargs(&buffer).unwrap();
                shown(arguments, environment, &names(&["mcp"])).1
            };
            assert_eq!(home(&["PATH=/usr/bin"]), Home::Default, "{cleared}");
            assert_eq!(
                home(&["PATH=/usr/bin", "COMMONMEASURE_HOME=/edge"]),
                Home::Set("/edge".into())
            );
            // The reviewer's interior-empty process: getenv finds the home
            // after the empty entry, and so does this.
            assert_eq!(
                home(&["OTHER=present", "", "COMMONMEASURE_HOME=/review/actual"]),
                Home::Set("/review/actual".into())
            );
            // Past an empty entry, without the variable: not a definite
            // unset.
            assert_eq!(home(&["OTHER=present", "", "MORE=1"]), Home::Unknown);
            // The reviewer's leading-empty case, with and without it.
            assert_eq!(
                home(&["", "OTHER=present", "COMMONMEASURE_HOME=/review/actual"]),
                Home::Set("/review/actual".into())
            );
            assert_eq!(home(&["", "OTHER=present"]), Home::Unknown);
            // An empty environment.
            assert_eq!(home(&[]), Home::Unknown, "{cleared}");
            // An entry named like a kernel key ends the environment early.
            // The lead's exact case: getenv finds the home after it, so the
            // end is in doubt and the home is not known.
            assert_eq!(
                home(&["OTHER=1", "", "pfz=x", "COMMONMEASURE_HOME=/a"]),
                Home::Unknown,
                "{cleared}"
            );
            assert_eq!(
                home(&[
                    "OTHER=1",
                    "",
                    "ptr_munge=x",
                    "MORE=1",
                    "COMMONMEASURE_HOME=/a"
                ]),
                Home::Unknown,
                "{cleared}"
            );
            // Without the variable anywhere it is unset, wherever the end
            // falls; and before the end, getenv finds it first.
            assert_eq!(home(&["OTHER=1", "", "pfz=x", "MORE=1"]), Home::Default);
            assert_eq!(
                home(&[
                    "COMMONMEASURE_HOME=/a",
                    "",
                    "pfz=x",
                    "COMMONMEASURE_HOME=/b"
                ]),
                Home::Set("/a".into())
            );
        }
        // A restricted process: the arguments and nothing after them.
        let restricted = procargs2("/bin/commonmeasure", &["/bin/commonmeasure", "mcp"], &[]);
        let (arguments, environment) = procargs(&restricted).unwrap();
        assert_eq!(
            shown(arguments, environment, &names(&["mcp"])),
            (Some("mcp".to_string()), Home::Unknown)
        );
        // Kernel strings this does not know stay after the empty entry, so
        // the environment's end is not shown.
        let mut unknown = procargs2("/bin/commonmeasure", &["/bin/commonmeasure"], &["A=1"]);
        unknown.extend_from_slice(b"future_key=1\0");
        let (arguments, environment) = procargs(&unknown).unwrap();
        assert_eq!(shown(arguments, environment, &[]), (None, Home::Unknown));
    }

    #[test]
    fn the_linux_environment_read_empty_is_unknown_and_an_empty_entry_is_not_its_end() {
        let home = |environ: &[u8]| {
            shown(
                proc_strings(b"/bin/commonmeasure\0mcp\0"),
                Some(proc_strings(environ)),
                &names(&["mcp"]),
            )
            .1
        };
        // Zero bytes: `linux_empty_probe.js`'s process.
        assert_eq!(home(b""), Home::Unknown);
        assert_eq!(home(b"PATH=/usr/bin\0"), Home::Default);
        assert_eq!(
            home(b"PATH=/usr/bin\0COMMONMEASURE_HOME=/edge\0"),
            Home::Set("/edge".into())
        );
        assert_eq!(
            home(b"OTHER=present\0\0COMMONMEASURE_HOME=/review/actual\0"),
            Home::Set("/review/actual".into())
        );
        assert_eq!(home(b"OTHER=present\0\0MORE=1\0"), Home::Unknown);
        assert_eq!(home(b"\0OTHER=present\0"), Home::Unknown);
        // An empty last entry ends the file: the environment was read whole.
        assert_eq!(home(b"OTHER=present\0\0"), Home::Default);
        // Set empty: the default Edge home.
        assert_eq!(home(b"COMMONMEASURE_HOME=\0"), Home::Default);
    }

    #[test]
    fn only_an_exact_subcommand_name_and_the_home_are_shown() {
        let subcommands = names(&["mcp", "serve", "relay"]);
        let arguments: [&[u8]; 4] = [b"mcp", b"--token=mcp", b"mcpx", b"relay"];
        let environment: [&[u8]; 2] = [b"TOKEN=secret", b"COMMONMEASURE_HOME=/edge"];
        // The program name is skipped even when it is a subcommand's name.
        assert_eq!(
            shown(arguments, Some(environment), &subcommands),
            (Some("relay".into()), Home::Set("/edge".into()))
        );
        let other: [&[u8]; 1] = [b"PATH=/usr/bin"];
        assert_eq!(
            shown([&b"x"[..], b"--token", b"abc"], Some(other), &subcommands),
            (None, Home::Default)
        );
        // Read, but empty: macOS returns a restricted process's arguments
        // without its environment, so this does not show the home unset.
        let empty: [&[u8]; 0] = [];
        assert_eq!(
            shown([&b"x"[..], b"serve"], Some(empty), &subcommands),
            (Some("serve".into()), Home::Unknown)
        );
        assert_eq!(
            shown([&b"x"[..], b"serve"], None::<[&[u8]; 0]>, &subcommands),
            (Some("serve".into()), Home::Unknown)
        );
    }

    #[test]
    fn an_executable_is_the_target_by_path_or_by_inode_and_unknown_without_either() {
        let target = Target {
            canonical: PathBuf::from("/bin/commonmeasure"),
            image: (1, 100),
            name: "commonmeasure".into(),
        };
        let at = |path: &str, images: &[((u64, u64), bool)]| Executable {
            path: (!path.is_empty()).then(|| PathBuf::from(path)),
            canonical: (!path.is_empty()).then(|| PathBuf::from(path)),
            images: images
                .iter()
                .map(|&(id, linked)| Image { id, linked })
                .collect(),
        };
        // Started from the file's path: an old inode renamed over is still
        // reported at that path.
        assert_eq!(
            at("/bin/commonmeasure", &[((1, 99), true)]).identify(&target),
            Identity::Target
        );
        // Through a hard link: another path, the same inode.
        assert_eq!(
            at("/opt/alias", &[((1, 100), true)]).identify(&target),
            Identity::Target
        );
        assert_eq!(
            at("", &[((1, 7), true), ((1, 100), true)]).identify(&target),
            Identity::Target
        );
        assert_eq!(
            at("/opt/other", &[((1, 99), true)]).identify(&target),
            Identity::Other
        );
        assert_eq!(at("/opt/other", &[]).identify(&target), Identity::Other);
        // No path, and every file it maps is linked somewhere else.
        assert_eq!(
            at("", &[((1, 7), true), ((1, 8), true)]).identify(&target),
            Identity::Other
        );
        // No path, and an image whose last link is gone: the reviewer's
        // hard-link alias sequence leaves exactly this.
        assert_eq!(
            at("", &[((1, 99), false), ((1, 8), true)]).identify(&target),
            Identity::Unknown
        );
        assert_eq!(at("", &[]).identify(&target), Identity::Unknown);
    }

    #[test]
    fn every_mapped_file_is_read_though_a_later_one_identifies_the_target() {
        // A library mapped executable before the main image: only the second
        // image is the target, so a walk that stopped at the first would
        // leave the process unidentified.
        let target = Target {
            canonical: PathBuf::from("/bin/commonmeasure"),
            image: (1, 100),
            name: "commonmeasure".into(),
        };
        let regions = [
            (0x1000, None),
            (0x2000, Some(((1, 7), false))),
            (0x3000, None),
            (0x4000, Some(((1, 100), true))),
        ];
        let walked = mapped(|address| {
            Ok::<_, ()>(
                regions
                    .iter()
                    .find(|(end, _)| *end > address)
                    .map(|&(end, image)| Region {
                        end: Some(end),
                        image: image.map(|(id, linked)| Image { id, linked }),
                    }),
            )
        })
        .unwrap();
        assert_eq!(
            walked.iter().map(|image| image.id).collect::<Vec<_>>(),
            [(1, 7), (1, 100)]
        );
        let executable = Executable {
            images: walked,
            ..Executable::default()
        };
        assert_eq!(executable.identify(&target), Identity::Target);
        // A region whose end overflows, or does not advance, ends the walk.
        let stuck = mapped(|_| {
            Ok::<_, ()>(Some(Region {
                end: None,
                image: Some(Image {
                    id: (1, 1),
                    linked: true,
                }),
            }))
        });
        assert_eq!(stuck.unwrap().len(), 1);
        assert_eq!(mapped(|_| Err::<Option<Region>, _>(7)).unwrap_err(), 7);
    }

    #[test]
    fn a_denied_process_is_another_user_s_only_when_its_owner_says_so() {
        let mine = |entry: Entry| match entry {
            Entry::Mine(mine) => Some((mine.parent, mine.name, mine.name_limit)),
            _ => None,
        };
        let read = |uid| Owner::Read {
            uid,
            parent: 1,
            name: "commonmeasure".into(),
        };
        assert!(matches!(denied(read(0), 501), Entry::Other));
        assert_eq!(
            mine(denied(read(501), 501)),
            Some((1, Some("commonmeasure".into()), MAXCOMLEN))
        );
        assert_eq!(mine(denied(Owner::Unreadable, 501)), Some((0, None, 0)));
        assert!(matches!(denied(Owner::Gone, 501), Entry::Gone));
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn the_owner_of_any_process_is_read_independently() {
        // launchd is root's, and proc_pidinfo describes it only to root.
        match table::owner(1) {
            Owner::Read { uid, parent, name } => {
                assert_eq!((uid, parent, name.as_str()), (0, 0, "launchd"));
            }
            _ => panic!("launchd's owner was not read"),
        }
        let pid = libc::c_int::try_from(std::process::id()).unwrap();
        // SAFETY: getuid and getppid cannot fail.
        let (uid, parent) = unsafe { (libc::getuid(), libc::getppid()) };
        match table::owner(pid) {
            Owner::Read {
                uid: read,
                parent: read_parent,
                ..
            } => {
                assert_eq!(read, uid);
                assert_eq!(read_parent, u32::try_from(parent).unwrap());
            }
            _ => panic!("this process's owner was not read"),
        }
        assert!(matches!(table::owner(i32::MAX), Owner::Gone));
    }

    #[test]
    fn the_console_is_admitted_only_as_launchd_s_child_started_before_launchd_was_asked() {
        let asked = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000);
        let again = || Some(7);
        let console = Console {
            pid: 7,
            asked,
            again: &again,
        };
        let process = |parent, started: Option<u64>| Mine {
            parent,
            started: started
                .map(|secs| SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs)),
            name: Some("commonmeasure".into()),
            name_limit: 31,
            executable: Executable::default(),
        };
        assert!(console.admits(7, &process(1, Some(999))));
        // Started after launchd answered: the pid was reused.
        assert!(!console.admits(7, &process(1, Some(1_000))));
        assert!(!console.admits(7, &process(1, Some(1_001))));
        // No start time, another parent, or another pid.
        assert!(!console.admits(7, &process(1, None)));
        assert!(!console.admits(7, &process(4242, Some(999))));
        assert!(!console.admits(8, &process(1, Some(999))));
    }

    #[test]
    fn an_unidentified_process_is_counted_by_its_name_or_when_it_has_none() {
        assert_eq!(
            unidentified(Some("commonmeasure".into()), "commonmeasure", 31),
            Some(Runs::Unidentified {
                name: Some("commonmeasure".into())
            })
        );
        assert_eq!(
            unidentified(Some("ssh-agent".into()), "commonmeasure", 31),
            None
        );
        // A denied process whose owner could not be read has no name either.
        assert_eq!(
            unidentified(None, "commonmeasure", 0),
            Some(Runs::Unidentified { name: None })
        );
    }

    #[test]
    fn a_process_name_matches_the_file_name_whole_or_truncated() {
        assert!(same_name("commonmeasure", "commonmeasure", 15));
        assert!(same_name("commonmeasure-d", "commonmeasure-dev", 15));
        assert!(!same_name("commonmeasure-", "commonmeasure-dev", 15));
        assert!(!same_name("common", "commonmeasure", 15));
        assert!(!same_name("", "commonmeasure", 15));
    }

    #[test]
    fn a_process_listing_that_fills_its_buffer_is_an_error() {
        let list = |filled: fn(usize) -> usize| {
            listed(move |buffer| match buffer {
                None => 3,
                Some(buffer) => {
                    let filled = filled(buffer.len());
                    for (slot, pid) in buffer.iter_mut().zip(1..).take(filled) {
                        *slot = pid;
                    }
                    i32::try_from(filled).unwrap()
                }
            })
        };
        assert_eq!(list(|_| 3).unwrap(), [1, 2, 3]);
        let error = list(|capacity| capacity).unwrap_err();
        assert!(error.contains("more than 259 processes"), "{error}");
        assert!(listed(|_| 0).is_err());
        assert!(listed(|buffer| if buffer.is_none() { 3 } else { 0 }).is_err());
    }

    /// A copy of this test binary under a name no other process has, to be
    /// started as a child that sleeps. Copies of the system's own `sleep`
    /// are killed on macOS soon after they start.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn sleeper(dir: &Path, tag: &str) -> PathBuf {
        let file = dir.join(format!("cm{}{tag}", std::process::id() % 100_000));
        std::fs::copy(std::env::current_exe().unwrap(), &file).unwrap();
        file
    }

    /// Start `program`, a copy of this test binary, running only
    /// `sleeping_child`, with `mcp` among its arguments and a
    /// credential-shaped variable first in its environment.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn start(program: &Path, arg0: Option<&str>, not_dumpable: bool) -> std::process::Child {
        start_with_home(program, arg0, not_dumpable, Some("/edge/a"))
    }

    fn start_with_home(
        program: &Path,
        arg0: Option<&str>,
        not_dumpable: bool,
        home: Option<&str>,
    ) -> std::process::Child {
        use std::os::unix::process::CommandExt as _;
        let mut command = std::process::Command::new(program);
        if let Some(arg0) = arg0 {
            command.arg0(arg0);
        }
        command
            .args(["--exact", "processes::tests::sleeping_child", "mcp"])
            .env_clear()
            .env("AAA_SENTINEL", "NOT_A_CREDENTIAL")
            .env("COMMONMEASURE_TEST_SLEEP", "1")
            .stdout(std::process::Stdio::null());
        if let Some(home) = home {
            command.env("COMMONMEASURE_HOME", home);
        }
        if not_dumpable {
            command.env("COMMONMEASURE_TEST_NOT_DUMPABLE", "1");
        }
        // On Linux, a child another test forks while this file is still open
        // for writing holds it open until it execs: ETXTBSY for a moment.
        let mut child = (0..50)
            .find_map(|_| match command.spawn() {
                Err(error) if error.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                    None
                }
                result => Some(result.unwrap()),
            })
            .expect("the file stayed busy");
        // Until the child has exec'd, the table shows this binary.
        std::thread::sleep(std::time::Duration::from_millis(500));
        assert!(child.try_wait().unwrap().is_none(), "the child runs");
        child
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn stop(mut child: std::process::Child) {
        child.kill().unwrap();
        child.wait().unwrap();
    }

    /// The child `start` runs: it sleeps, first making itself not dumpable
    /// when asked, as `ssh-agent` does. A no-op in an ordinary test run.
    #[test]
    fn sleeping_child() {
        if std::env::var_os("COMMONMEASURE_TEST_SLEEP").is_none() {
            return;
        }
        #[cfg(target_os = "linux")]
        if std::env::var_os("COMMONMEASURE_TEST_NOT_DUMPABLE").is_some() {
            // SAFETY: prctl with PR_SET_DUMPABLE takes integers only.
            assert_eq!(unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) }, 0);
        }
        std::thread::sleep(std::time::Duration::from_secs(30));
    }

    #[test]
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn a_process_running_the_file_is_found_with_its_home_and_no_other_text() {
        let dir = tempfile::tempdir().unwrap();
        let file = sleeper(dir.path(), "a");
        // Argument zero empty and a credential-shaped variable first in the
        // environment, the shape that once leaked it.
        let child = start(&file, Some(""), false);
        let pid = child.id();
        let found = running(&file, None, &names(&["mcp", "serve"]));
        // Exempt as the console only when its parent is launchd, which a
        // child of this test is not.
        let again = || Some(pid);
        let console = Console {
            pid,
            asked: SystemTime::now(),
            again: &again,
        };
        let as_console = running(&file, Some(&console), &[]);
        stop(child);

        let found = found.unwrap();
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].pid, pid);
        assert!(
            matches!(&found[0].runs, Runs::File { path: Some(path) }
                if path.canonicalize().unwrap() == file.canonicalize().unwrap()),
            "{found:?}"
        );
        assert_eq!(found[0].subcommand.as_deref(), Some("mcp"));
        assert_eq!(found[0].home, Home::Set("/edge/a".into()));
        assert!(!format!("{found:?}").contains("NOT_A_CREDENTIAL"));
        assert_eq!(as_console.unwrap().len(), 1);
        assert_eq!(running(&file, None, &[]).unwrap(), vec![]);
    }

    #[test]
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn a_process_started_without_the_home_reads_as_the_default() {
        // On macOS the kernel's own strings follow the environment in
        // `KERN_PROCARGS2`, after a run of NULs; they do not make the home
        // unknown.
        let dir = tempfile::tempdir().unwrap();
        let file = sleeper(dir.path(), "f");
        let child = start_with_home(&file, None, false, None);
        let found = running(&file, None, &[]);
        stop(child);
        let found = found.unwrap();
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].home, Home::Default);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn a_process_with_no_environment_reads_as_unknown() {
        // The reviewer's `linux_empty_probe.js`: zero bytes in
        // `/proc/<pid>/environ` do not show the variable unset.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("sleep");
        std::fs::copy("/bin/sleep", &file).unwrap();
        let mut child = std::process::Command::new(&file)
            .arg("30")
            .env_clear()
            .spawn()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let found = running(&file, None, &[]);
        child.kill().unwrap();
        child.wait().unwrap();
        let found = found.unwrap();
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].home, Home::Unknown);
    }

    #[test]
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn a_process_started_through_a_hard_link_runs_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = sleeper(dir.path(), "b");
        let alias = dir.path().join("alias");
        std::fs::hard_link(&file, &alias).unwrap();
        let child = start(&alias, None, false);
        let pid = child.id();
        let found = running(&file, None, &[]);
        stop(child);
        let found = found.unwrap();
        assert_eq!(
            found.iter().map(|process| process.pid).collect::<Vec<_>>(),
            [pid]
        );
        assert!(matches!(found[0].runs, Runs::File { .. }), "{found:?}");
    }

    #[test]
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn a_process_whose_image_lost_its_last_link_is_still_found() {
        // The reviewer's sequence: start the file, link an alias to it,
        // rename a fresh copy over the file, remove the alias. The process
        // runs on an inode with no name left.
        let dir = tempfile::tempdir().unwrap();
        let file = sleeper(dir.path(), "c");
        let child = start(&file, None, false);
        let pid = child.id();
        let alias = dir.path().join("alias");
        std::fs::hard_link(&file, &alias).unwrap();
        let fresh = dir.path().join("fresh");
        std::fs::copy(&file, &fresh).unwrap();
        std::fs::rename(&fresh, &file).unwrap();
        std::fs::remove_file(&alias).unwrap();
        let found = running(&file, None, &[]);
        stop(child);

        let found = found.unwrap();
        assert_eq!(
            found.iter().map(|process| process.pid).collect::<Vec<_>>(),
            [pid],
            "{found:?}"
        );
        if cfg!(target_os = "macos") {
            // proc_pidpath fails, and the image has no link: known by name.
            assert_eq!(
                found[0].runs,
                Runs::Unidentified {
                    name: Some(file.file_name().unwrap().to_string_lossy().into())
                }
            );
        } else {
            // /proc/<pid>/exe still reads the path it was started from.
            assert!(matches!(found[0].runs, Runs::File { .. }), "{found:?}");
        }
        assert_eq!(found[0].home, Home::Set("/edge/a".into()));
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn a_process_that_is_not_dumpable_is_found_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let file = sleeper(dir.path(), "d");
        let child = start(&file, None, true);
        let pid = child.id();
        let unreadable = std::fs::read_link(format!("/proc/{pid}/exe")).unwrap_err();
        let found = running(&file, None, &[]);
        stop(child);

        assert_eq!(unreadable.kind(), std::io::ErrorKind::PermissionDenied);
        let found = found.unwrap();
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].pid, pid);
        assert_eq!(
            found[0].runs,
            Runs::Unidentified {
                name: Some(file.file_name().unwrap().to_string_lossy().into())
            }
        );
        assert_eq!(found[0].home, Home::Unknown);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn an_orphan_at_the_console_s_pid_is_exempt_only_as_the_process_launchd_reported() {
        // The reviewer's orphan probe: a copy started through a shell that
        // exits at once, so its parent is launchd, as the console's is.
        let dir = tempfile::tempdir().unwrap();
        let file = sleeper(dir.path(), "e");
        let before_start = SystemTime::now();
        std::thread::sleep(std::time::Duration::from_millis(50));
        let output = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(r#"COMMONMEASURE_TEST_SLEEP=1 "$0" --exact processes::tests::sleeping_child >/dev/null 2>&1 & echo $!"#)
            .arg(&file)
            .output()
            .unwrap();
        let pid: u32 = String::from_utf8(output.stdout)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(500));
        let after_start = SystemTime::now();
        let parent = std::process::Command::new("ps")
            .args(["-o", "ppid=", "-p", &pid.to_string()])
            .output()
            .unwrap();
        let scan = |asked, again: Option<u32>| {
            let again = move || again;
            let console = Console {
                pid,
                asked,
                again: &again,
            };
            running(&file, Some(&console), &[]).map(|found| {
                found
                    .iter()
                    .map(|process| (process.pid, process.console_changed))
                    .collect::<Vec<_>>()
            })
        };
        let reused = scan(before_start, Some(pid));
        let reported = scan(after_start, Some(pid));
        let changed = scan(after_start, Some(pid + 1));
        let gone = scan(after_start, None);
        // SAFETY: kill with a pid and a signal number.
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };

        assert_eq!(String::from_utf8_lossy(&parent.stdout).trim(), "1");
        // Started after launchd was asked: a reused pid, counted, and marked
        // as a console that changed during the check.
        assert_eq!(reused.unwrap(), [(pid, true)]);
        // Started before, and launchd names it again: the console.
        assert_eq!(reported.unwrap(), Vec::<(u32, bool)>::new());
        // Launchd names another pid, or none, the second time: counted.
        assert_eq!(changed.unwrap(), [(pid, true)]);
        assert_eq!(gone.unwrap(), [(pid, true)]);
    }
}
