use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::thread::ThreadId;
use std::time::Duration;

use tokio::sync::oneshot;

const DEFAULT_TEST_TIMEOUT_SECS: u64 = 20;

fn test_timeout_secs() -> u64 {
    static SECS: LazyLock<u64> = LazyLock::new(|| {
        std::env::var("KRAMLI_TEST_TIMEOUT_SECS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(DEFAULT_TEST_TIMEOUT_SECS)
    });
    *SECS
}

pub(crate) fn test_timeout() -> Duration {
    Duration::from_secs(test_timeout_secs())
}

pub(crate) fn test_timeout_arg() -> String {
    format!("{}s", test_timeout_secs())
}

pub(crate) fn run_sync_test<T, F>(name: &str, f: F) -> T
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        let _ = tx.send(result);
    });
    match rx.recv_timeout(test_timeout()) {
        Ok(Ok(value)) => value,
        Ok(Err(payload)) => std::panic::resume_unwind(payload),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            panic!("test {name} timed out after {:?}", test_timeout());
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            panic!("test {name} exited without reporting a result");
        }
    }
}

pub(crate) async fn run_async_test<T, F>(name: &str, future: F) -> T
where
    F: Future<Output = T>,
{
    match tokio::time::timeout(test_timeout(), future).await {
        Ok(value) => value,
        Err(_) => panic!("test {name} timed out after {:?}", test_timeout()),
    }
}

pub(crate) static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static ENV_SCOPE_SEQUENCE: AtomicU64 = AtomicU64::new(1);

tokio::task_local! {
    static ENV_SCOPE_ID: u64;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EnvOwner {
    Async(u64),
    Thread(ThreadId),
}

impl EnvOwner {
    fn matches_current(self) -> bool {
        match self {
            Self::Async(owner) => ENV_SCOPE_ID.try_with(|current| *current == owner) == Ok(true),
            Self::Thread(owner) => owner == std::thread::current().id(),
        }
    }
}

static ENV_OWNER: Mutex<Option<EnvOwner>> = Mutex::new(None);

struct EnvOwnerGuard;

impl EnvOwnerGuard {
    fn claim(owner: EnvOwner) -> Self {
        *ENV_OWNER.lock().expect("test env owner lock poisoned") = Some(owner);
        Self
    }
}

impl Drop for EnvOwnerGuard {
    fn drop(&mut self) {
        *ENV_OWNER.lock().expect("test env owner lock poisoned") = None;
    }
}

pub(crate) fn env_access_allowed() -> bool {
    ENV_OWNER
        .lock()
        .expect("test env owner lock poisoned")
        .is_none_or(|owner| owner.matches_current())
}

type MockServerRegistration = (String, oneshot::Sender<()>);
static MOCK_SERVER_READY: LazyLock<Mutex<Vec<MockServerRegistration>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

pub(crate) fn register_mock_server(base_url: String) -> oneshot::Receiver<()> {
    let (sender, receiver) = oneshot::channel();
    MOCK_SERVER_READY
        .lock()
        .expect("mock server readiness lock poisoned")
        .push((base_url, sender));
    receiver
}

pub(crate) fn notify_mock_server_ready(url: &str) {
    let mut registrations = MOCK_SERVER_READY
        .lock()
        .expect("mock server readiness lock poisoned");
    let mut index = 0;
    while index < registrations.len() {
        if url.starts_with(&registrations[index].0) {
            let (_, sender) = registrations.swap_remove(index);
            let _ = sender.send(());
        } else {
            index += 1;
        }
    }
}

struct EnvRestore {
    previous: Vec<(String, Option<String>)>,
}

impl EnvRestore {
    fn set(vars: &[(&str, &str)]) -> Self {
        let previous = vars
            .iter()
            .map(|(key, _)| ((*key).to_string(), std::env::var(key).ok()))
            .collect::<Vec<_>>();
        for (key, value) in vars {
            std::env::set_var(key, value);
            notify_mock_server_ready(value);
        }
        Self { previous }
    }
}

impl Drop for EnvRestore {
    fn drop(&mut self) {
        for (key, value) in &self.previous {
            if let Some(value) = value {
                std::env::set_var(key, value);
            } else {
                std::env::remove_var(key);
            }
        }
    }
}

pub(crate) fn with_env_lock<T>(f: impl FnOnce() -> T) -> T {
    let _guard = ENV_LOCK.blocking_lock();
    let _owner = EnvOwnerGuard::claim(EnvOwner::Thread(std::thread::current().id()));
    f()
}

pub(crate) async fn with_env_lock_async<T, Fut>(f: impl FnOnce() -> Fut) -> T
where
    Fut: Future<Output = T>,
{
    let _guard = ENV_LOCK.lock().await;
    let scope_id = ENV_SCOPE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    ENV_SCOPE_ID
        .scope(scope_id, async {
            let _owner = EnvOwnerGuard::claim(EnvOwner::Async(scope_id));
            f().await
        })
        .await
}

pub(crate) fn with_env_vars<T>(vars: &[(&str, &str)], f: impl FnOnce() -> T) -> T {
    with_env_lock(|| {
        let _restore = EnvRestore::set(vars);
        f()
    })
}

pub(crate) async fn with_env_vars_async<T, Fut>(vars: &[(&str, &str)], f: impl FnOnce() -> Fut) -> T
where
    Fut: Future<Output = T>,
{
    let _guard = ENV_LOCK.lock().await;
    let scope_id = ENV_SCOPE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    ENV_SCOPE_ID
        .scope(scope_id, async {
            let _owner = EnvOwnerGuard::claim(EnvOwner::Async(scope_id));
            let _restore = EnvRestore::set(vars);
            f().await
        })
        .await
}

/// Run an ignored unit test under a pseudo-TTY via `script`.
///
/// Uses GNU `script -c` (+ optional `timeout`) on Linux, and BSD
/// `script file command args...` on macOS/BSD.
pub(crate) fn run_test_in_pseudo_terminal(test_filter: &str) {
    let exe = std::env::current_exe().expect("current test executable should be available");
    let script = ["/usr/bin/script", "/bin/script"]
        .into_iter()
        .find(|path| std::path::Path::new(path).exists())
        .expect("script binary should exist for pseudo-terminal coverage");

    let status = if cfg!(any(
        target_os = "macos",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd"
    )) {
        std::process::Command::new(script)
            .arg("-q")
            .arg("/dev/null")
            .arg(&exe)
            .args([
                test_filter,
                "--exact",
                "--nocapture",
                "--test-threads=1",
                "--ignored",
            ])
            .status()
            .expect("pseudo-terminal subprocess should spawn")
    } else {
        let command = format!(
            "{} {test_filter} --exact --nocapture --test-threads=1 --ignored",
            exe.display()
        );
        let timeout = ["timeout", "gtimeout"]
            .into_iter()
            .find(|name| std::process::Command::new(name).arg("--version").output().is_ok());
        let mut child = if let Some(timeout) = timeout {
            let mut cmd = std::process::Command::new(timeout);
            cmd.args([&test_timeout_arg(), script, "-q", "-c", &command, "/dev/null"]);
            cmd
        } else {
            let mut cmd = std::process::Command::new(script);
            cmd.args(["-q", "-c", &command, "/dev/null"]);
            cmd
        };
        child
            .status()
            .expect("pseudo-terminal subprocess should spawn")
    };

    assert!(
        status.success(),
        "pseudo-terminal test {test_filter} failed with {status:?}"
    );
}

#[kramli_test_macros::tokio_test]
async fn env_owner_rejects_foreign_tasks() {
    with_env_lock_async(|| async {
        assert!(env_access_allowed());
        assert!(!tokio::spawn(async { env_access_allowed() }).await.unwrap());
    })
    .await;
}
