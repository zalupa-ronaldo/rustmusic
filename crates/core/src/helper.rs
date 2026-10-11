//! Keep forked resolver/runtime processes inside the lifetime of their operation.
use anyhow::Result;
use std::process::Output;
use tokio::process::Command;

pub(crate) async fn output(command: &mut Command) -> Result<Output> {
    #[cfg(unix)]
    command.process_group(0);
    command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let child = command.spawn()?;
    #[cfg(unix)]
    let _tree = Tree(child.id().map(|pid| pid as i32));
    #[cfg(windows)]
    let _tree = Tree::attach(&child)?;
    Ok(child.wait_with_output().await?)
}
#[cfg(unix)]
struct Tree(Option<i32>);
#[cfg(unix)]
impl Drop for Tree {
    fn drop(&mut self) {
        if let Some(pid) = self.0 {
            // Negative PID addresses only the process group created for this helper.
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
    }
}
#[cfg(windows)]
struct Tree(isize);
#[cfg(windows)]
impl Tree {
    fn attach(child: &tokio::process::Child) -> Result<Self> {
        use windows_sys::Win32::System::JobObjects::*;
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        anyhow::ensure!(!handle.is_null(), "Cannot create helper process job");
        let job = Self(handle as isize);
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let configured = unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of_val(&limits) as u32,
            )
        };
        anyhow::ensure!(configured != 0, "Cannot configure helper process job");
        let process = child
            .raw_handle()
            .ok_or_else(|| anyhow::anyhow!("Helper already exited"))?;
        anyhow::ensure!(
            unsafe { AssignProcessToJobObject(handle, process) } != 0,
            "Cannot attach helper process job"
        );
        Ok(job)
    }
}
#[cfg(windows)]
impl Drop for Tree {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0 as _);
        }
    }
}
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[tokio::test]
    async fn cancellation_stops_forked_helpers() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut command = Command::new("sh");
        command
            .args([
                "-c",
                "sleep 60 & child=$!; printf '%s' \"$child\" > \"$1\"; wait",
                "sh",
            ])
            .arg(file.path());
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(400), output(&mut command))
                .await
                .is_err()
        );
        let pid = std::fs::read_to_string(file.path()).unwrap();
        assert!(!pid.is_empty());
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let state = std::process::Command::new("ps")
            .args(["-p", &pid, "-o", "stat="])
            .output()
            .unwrap();
        let state = String::from_utf8_lossy(&state.stdout);
        assert!(
            state.trim().is_empty() || state.trim().starts_with('Z'),
            "Forked helper still running: {state}"
        );
    }
}
