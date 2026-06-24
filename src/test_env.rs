use std::future::Future;

pub(crate) static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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
    f()
}

pub(crate) async fn with_env_lock_async<T, Fut>(f: impl FnOnce() -> Fut) -> T
where
    Fut: Future<Output = T>,
{
    let _guard = ENV_LOCK.lock().await;
    f().await
}

pub(crate) fn with_env_vars<T>(vars: &[(&str, &str)], f: impl FnOnce() -> T) -> T {
    with_env_lock(|| {
        let _restore = EnvRestore::set(vars);
        f()
    })
}

pub(crate) async fn with_env_vars_async<T, Fut>(
    vars: &[(&str, &str)],
    f: impl FnOnce() -> Fut,
) -> T
where
    Fut: Future<Output = T>,
{
    let _guard = ENV_LOCK.lock().await;
    let _restore = EnvRestore::set(vars);
    f().await
}
