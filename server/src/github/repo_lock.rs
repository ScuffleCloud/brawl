use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use octocrab::models::RepositoryId;
use parking_lot::Mutex;
use tokio::sync::OwnedSemaphorePermit;

#[derive(Debug, Clone)]
pub struct RepoLock {
    active_repos: Arc<Mutex<HashMap<RepositoryId, Lock>>>,
}

#[derive(Debug, Clone)]
struct Lock {
    count: Arc<AtomicUsize>,
    semaphore: Arc<tokio::sync::Semaphore>,
}

pub struct LockGuard {
    _inner: LockGuardInner,
    _permit: OwnedSemaphorePermit,
}

struct LockGuardInner {
    repo_id: RepositoryId,
    count: Arc<AtomicUsize>,
    repo_lock: RepoLock,
}

impl LockGuardInner {
    fn new(repo_id: RepositoryId, count: Arc<AtomicUsize>, repo_lock: RepoLock) -> Self {
        count.fetch_add(1, Ordering::Relaxed);
        Self {
            repo_id,
            count,
            repo_lock,
        }
    }

    fn into_guard(self, permit: OwnedSemaphorePermit) -> LockGuard {
        LockGuard {
            _inner: self,
            _permit: permit,
        }
    }
}

impl Drop for LockGuardInner {
    fn drop(&mut self) {
        if self.count.fetch_sub(1, Ordering::Relaxed) == 1 {
            let mut lock = self.repo_lock.active_repos.lock();
            // The reason we do an extra load here is that the mutex is required to do a
            // increment, therefore its possible that the count might have been
            // incremented by another thread after we decremented it to 0, but
            // before we could aquire the lock. So the additional check is to ensure we
            // the count is 0 before deleting the entry.
            if self.count.load(Ordering::Relaxed) == 0 {
                lock.remove(&self.repo_id);
            }
        }
    }
}

impl Lock {
    fn new() -> Self {
        Self {
            count: Arc::new(AtomicUsize::new(0)),
            semaphore: Arc::new(tokio::sync::Semaphore::new(1)),
        }
    }
}

impl Default for RepoLock {
    fn default() -> Self {
        Self::new()
    }
}

impl RepoLock {
    pub fn new() -> Self {
        Self {
            active_repos: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn lock(&self, repo_id: RepositoryId) -> LockGuard {
        let (inner, semaphore) = {
            let mut lock = self.active_repos.lock();
            let lock = lock.entry(repo_id).or_insert_with(Lock::new);
            let inner = LockGuardInner::new(repo_id, lock.count.clone(), self.clone());
            (inner, lock.semaphore.clone())
        };

        inner.into_guard(semaphore.acquire_owned().await.expect("semaphore has no permits"))
    }
}
