//! Process ownership via Windows Job Objects (ADR 0006).
//!
//! Each service runs in its own *named* job (`Local\Devluma-<service id>`), created without
//! `KILL_ON_JOB_CLOSE` so services survive a Devluma crash and can be re-adopted. The child
//! is created suspended, assigned to the job, then resumed, so every process it spawns is
//! inside the job from the first instruction. Stopping terminates the job — the owned tree —
//! never processes found by name.

use std::ffi::OsStr;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::Duration;

use windows::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_BASIC_PROCESS_ID_LIST, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectBasicProcessIdList, JobObjectExtendedLimitInformation, OpenJobObjectW,
    QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
};
use windows::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, OpenThread, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, QueryFullProcessImageNameW,
    ResumeThread, THREAD_SUSPEND_RESUME, WaitForSingleObject,
};
use windows::core::{PCWSTR, PWSTR};

const CREATE_SUSPENDED: u32 = 0x0000_0004;
/// `JOB_OBJECT_ALL_ACCESS` (winnt.h).
const JOB_OBJECT_ALL_ACCESS: u32 = 0x001F_001F;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

fn err(e: windows::core::Error) -> std::io::Error {
    std::io::Error::other(e.message())
}

/// An owned kernel handle, closed on drop.
#[derive(Debug)]
struct Owned(HANDLE);

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: the handle was returned by a successful Win32 call and is closed exactly once.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

// SAFETY: kernel handles may be used from any thread.
unsafe impl Send for Owned {}
unsafe impl Sync for Owned {}

#[derive(Debug)]
pub struct Job {
    handle: Owned,
}

/// A fresh job name for one run of a service. Never reused: a terminated job can't accept
/// new processes.
pub fn new_job_name(service_id: &str) -> String {
    format!("Local\\Devluma-{service_id}-{}", ulid::Ulid::generate())
}

impl Job {
    /// Creates the named job.
    pub fn create(name: &str) -> std::io::Result<Self> {
        let name = wide(OsStr::new(name));
        // SAFETY: `name` is a NUL-terminated UTF-16 string that outlives the call.
        let handle = unsafe { CreateJobObjectW(None, PCWSTR(name.as_ptr())) }.map_err(err)?;
        Ok(Self {
            handle: Owned(handle),
        })
    }

    /// Opens an existing job (re-adoption after a restart). `None` if it no longer exists.
    pub fn open(name: &str) -> Option<Self> {
        let name = wide(OsStr::new(name));
        // SAFETY: as above.
        let handle =
            unsafe { OpenJobObjectW(JOB_OBJECT_ALL_ACCESS, false, PCWSTR(name.as_ptr())) }.ok()?;
        Some(Self {
            handle: Owned(handle),
        })
    }

    /// Starts `cmd` suspended, assigns it to this job, then resumes it.
    pub fn spawn(&self, cmd: &mut Command) -> std::io::Result<Child> {
        cmd.creation_flags(CREATE_SUSPENDED | CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
        let mut child = cmd.spawn()?;
        let process = HANDLE(child.as_raw_handle());
        // SAFETY: `process` is the live handle owned by `child`.
        if let Err(e) = unsafe { AssignProcessToJobObject(self.handle.0, process) } {
            let _ = child.kill();
            let _ = child.wait();
            return Err(err(e));
        }
        if let Err(e) = resume_threads(child.id()) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(e);
        }
        Ok(child)
    }

    /// Process ids currently in the job.
    pub fn process_ids(&self) -> std::io::Result<Vec<u32>> {
        // Header + room for 256 ids.
        #[repr(C)]
        struct List {
            header: JOBOBJECT_BASIC_PROCESS_ID_LIST,
            rest: [usize; 255],
        }
        let mut list: List = unsafe { std::mem::zeroed() };
        // SAFETY: `list` is a writable buffer of the size passed.
        unsafe {
            QueryInformationJobObject(
                Some(self.handle.0),
                JobObjectBasicProcessIdList,
                &mut list as *mut List as *mut _,
                size_of::<List>() as u32,
                None,
            )
        }
        .map_err(err)?;
        let n = list.header.NumberOfProcessIdsInList as usize;
        let ids: &[usize] = unsafe {
            // SAFETY: the API wrote `n` (≤ 256) ids starting at `ProcessIdList`.
            std::slice::from_raw_parts(list.header.ProcessIdList.as_ptr(), n.min(256))
        };
        Ok(ids.iter().map(|&id| id as u32).collect())
    }

    /// Makes the job end its processes when its last handle closes — i.e. when Devluma exits
    /// or crashes. Used for tunnels, which must never keep publishing without Devluma.
    pub fn kill_on_close(&self) -> std::io::Result<()> {
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: `info` is a valid, initialized structure of the size passed.
        unsafe {
            SetInformationJobObject(
                self.handle.0,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        }
        .map_err(err)
    }

    /// Terminates every process in the job (the service's owned process tree).
    pub fn terminate(&self, exit_code: u32) -> std::io::Result<()> {
        // SAFETY: valid job handle.
        unsafe { TerminateJobObject(self.handle.0, exit_code) }.map_err(err)
    }

    /// Waits until the job has no processes left.
    pub fn wait_empty(&self, timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            match self.process_ids() {
                Ok(ids) if ids.is_empty() => return true,
                Err(_) => return false,
                _ => {}
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// Resumes every thread of a just-created suspended process.
fn resume_threads(pid: u32) -> std::io::Result<()> {
    // SAFETY: snapshot + iteration per the ToolHelp contract; handles are closed via `Owned`.
    unsafe {
        let snap = Owned(CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0).map_err(err)?);
        let mut entry = THREADENTRY32 {
            dwSize: size_of::<THREADENTRY32>() as u32,
            ..Default::default()
        };
        let mut resumed = 0;
        let mut ok = Thread32First(snap.0, &mut entry).is_ok();
        while ok {
            if entry.th32OwnerProcessID == pid {
                let thread = Owned(
                    OpenThread(THREAD_SUSPEND_RESUME, false, entry.th32ThreadID).map_err(err)?,
                );
                ResumeThread(thread.0);
                resumed += 1;
            }
            ok = Thread32Next(snap.0, &mut entry).is_ok();
        }
        if resumed == 0 {
            return Err(std::io::Error::other("no threads found to resume"));
        }
    }
    Ok(())
}

/// Identity of a running process, used to prove ownership (PID reuse protection).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessIdentity {
    pub pid: u32,
    /// FILETIME as u64 (100 ns since 1601), stable for the life of the process.
    pub created: u64,
    pub image: PathBuf,
}

fn open_query(pid: u32) -> Option<Owned> {
    // SAFETY: plain OpenProcess; failure (process gone, access denied) yields None.
    unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            false,
            pid,
        )
    }
    .ok()
    .map(Owned)
}

pub fn identity(pid: u32) -> Option<ProcessIdentity> {
    let h = open_query(pid)?;
    let (mut created, mut exit, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    let mut buf = vec![0u16; 1024];
    let mut len = buf.len() as u32;
    // SAFETY: valid process handle; out-params point to live locals / buffer of `len` u16s.
    unsafe {
        GetProcessTimes(h.0, &mut created, &mut exit, &mut kernel, &mut user).ok()?;
        QueryFullProcessImageNameW(h.0, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len)
            .ok()?;
    }
    if is_exited(&h) {
        return None;
    }
    Some(ProcessIdentity {
        pid,
        created: ((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64,
        image: PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])),
    })
}

fn is_exited(h: &Owned) -> bool {
    // SAFETY: valid handle with SYNCHRONIZE access; zero timeout.
    unsafe { WaitForSingleObject(h.0, 0) == WAIT_OBJECT_0 }
}

/// Blocks until process `pid` exits (or returns immediately if it's gone).
pub fn wait_for_exit(pid: u32, timeout: Option<Duration>) -> bool {
    let Some(h) = open_query(pid) else {
        return true;
    };
    let ms = timeout.map_or(u32::MAX, |t| t.as_millis().min(u32::MAX as u128 - 1) as u32);
    // SAFETY: valid handle with SYNCHRONIZE access.
    unsafe { WaitForSingleObject(h.0, ms) == WAIT_OBJECT_0 }
}

/// True if `path_a` and `path_b` name the same file (case-insensitive, normalized).
pub fn same_image(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        std::fs::canonicalize(p)
            .unwrap_or_else(|_| p.to_path_buf())
            .to_string_lossy()
            .trim_start_matches(r"\\?\")
            .to_lowercase()
    };
    norm(a) == norm(b)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Test helper processes exit as soon as the process that started them is gone, so an
    /// aborted test run can never leave them behind (they'd also lock the test binary).
    pub(crate) fn exit_with_parent() {
        if let Some(pid) = std::env::var("DEVLUMA_TEST_PARENT")
            .ok()
            .and_then(|p| p.parse::<u32>().ok())
        {
            std::thread::spawn(move || {
                wait_for_exit(pid, None);
                std::process::exit(0);
            });
        }
    }

    /// A command re-running this test binary as `test_name`, tied to the current process.
    pub(crate) fn helper_command(test_name: &str) -> Command {
        let mut cmd = Command::new(std::env::current_exe().unwrap());
        cmd.args(["--exact", test_name, "--ignored", "--nocapture"])
            .env("DEVLUMA_TEST_PARENT", std::process::id().to_string());
        cmd
    }

    /// The test binary re-runs itself as a child that spawns a grandchild and sleeps, giving a
    /// real process tree to own and terminate.
    fn tree_command() -> Command {
        let mut cmd = helper_command("platform::windows::job::tests::tree_helper");
        cmd.env("DEVLUMA_TREE_HELPER", "1");
        cmd
    }

    #[test]
    #[ignore = "helper process for job tests"]
    #[allow(
        clippy::zombie_processes,
        reason = "the grandchild is deliberately left running: it's what the job must own"
    )]
    fn tree_helper() {
        exit_with_parent();
        if std::env::var_os("DEVLUMA_TREE_HELPER").is_some() {
            let _child = helper_command("platform::windows::job::tests::sleep_helper")
                .spawn()
                .unwrap();
        }
        std::thread::sleep(Duration::from_secs(60));
    }

    #[test]
    #[ignore = "helper process for job tests"]
    fn sleep_helper() {
        exit_with_parent();
        std::thread::sleep(Duration::from_secs(60));
    }

    #[test]
    fn owns_and_terminates_the_whole_tree() {
        let id = new_job_name("test");
        let job = Job::create(&id).unwrap();
        let mut child = job.spawn(&mut tree_command()).unwrap();

        // Wait for the grandchild to join the job.
        let mut ids = Vec::new();
        for _ in 0..200 {
            ids = job.process_ids().unwrap();
            if ids.len() >= 2 {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(
            ids.len() >= 2,
            "child and grandchild must both be in the job: {ids:?}"
        );
        assert!(ids.contains(&child.id()));

        let ident = identity(child.id()).unwrap();
        assert!(same_image(&ident.image, &std::env::current_exe().unwrap()));
        assert_eq!(
            identity(child.id()).unwrap().created,
            ident.created,
            "creation time is stable"
        );

        // Re-open by name, as after a restart, and terminate the tree.
        let reopened = Job::open(&id).expect("job still exists while processes run");
        reopened.terminate(1).unwrap();
        assert!(reopened.wait_empty(Duration::from_secs(10)));
        assert!(child.wait().is_ok());
        assert!(identity(child.id()).is_none());
    }

    #[test]
    fn unrelated_processes_are_not_in_the_job() {
        let job = Job::create(&new_job_name("test")).unwrap();
        assert!(job.process_ids().unwrap().is_empty());
        assert!(!job.process_ids().unwrap().contains(&std::process::id()));
    }
}
