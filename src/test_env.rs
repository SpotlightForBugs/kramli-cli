use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::thread::ThreadId;

use tokio::sync::oneshot;

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

#[tokio::test]
async fn env_owner_rejects_foreign_tasks() {
    with_env_lock_async(|| async {
        assert!(env_access_allowed());
        assert!(!tokio::spawn(async { env_access_allowed() }).await.unwrap());
    })
    .await;
}
