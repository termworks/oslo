//! Watched child process groups and bounded termination.

use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use std::io;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

pub struct ChildGroup {
    child: Child,
}

impl ChildGroup {
    pub fn spawn(argv: &[String], root: &std::path::Path) -> io::Result<Self> {
        let (program, args) = argv
            .split_first()
            .ok_or_else(|| io::Error::other("watch needs a command"))?;
        let mut command = Command::new(program);
        command
            .args(args)
            .current_dir(root)
            .env_remove("OSLO_WATCH_WORKER")
            .env_remove("OSLO_WATCH_SCRATCH_BOOTSTRAP")
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        command.process_group(0);
        command.spawn().map(|child| Self { child })
    }

    pub fn id(&self) -> u32 {
        self.child.id()
    }

    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }

    pub fn terminate(
        &mut self,
        grace: Duration,
        mut between: impl FnMut(),
    ) -> io::Result<ExitStatus> {
        self.send(Signal::SIGTERM);
        let deadline = Instant::now() + grace;
        loop {
            if let Some(status) = self.child.try_wait()? {
                return Ok(status);
            }
            between();
            if Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        self.send(Signal::SIGKILL);
        self.child.wait()
    }

    fn send(&self, signal: Signal) {
        let group = Pid::from_raw(-(self.child.id() as i32));
        let _ = kill(group, signal);
    }
}

pub fn status_code(status: ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(0))
}
