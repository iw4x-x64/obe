use crate::social::friends;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::watch;

pub const ONLINE_FOR: Duration = Duration::from_secs(45);
pub const MAX_WAIT: Duration = Duration::from_secs(30);

static REVISIONS: LazyLock<Mutex<HashMap<u64, u64>>> = LazyLock::new(Default::default);

static EPOCH: LazyLock<u64> =
    LazyLock::new(|| (chrono::Utc::now().timestamp_millis() as u64) << 16);

static TICK: LazyLock<watch::Sender<u64>> = LazyLock::new(|| watch::channel(0).0);

static SEEN: LazyLock<Mutex<HashMap<u64, Instant>>> = LazyLock::new(Default::default);

pub fn revision(user: u64) -> u64 {
    REVISIONS
        .lock()
        .unwrap()
        .get(&user)
        .copied()
        .unwrap_or(*EPOCH)
}

pub fn bump(users: &[u64]) {
    {
        let mut revisions = REVISIONS.lock().unwrap();

        for user in users {
            *revisions.entry(*user).or_insert(*EPOCH) += 1;
        }
    }

    TICK.send_modify(|t| *t = t.wrapping_add(1));
}

pub fn watchers_of(user: u64) -> Vec<u64> {
    let mut out: Vec<u64> = friends::list(user).iter().map(|f| f.user_id).collect();

    out.extend(friends::incoming_requests(user).iter().map(|f| f.user_id));
    out.extend(friends::outgoing_requests(user).iter().map(|f| f.user_id));
    out.sort_unstable();
    out.dedup();
    out
}

pub fn bump_watchers_of(user: u64) {
    let watchers = watchers_of(user);

    if !watchers.is_empty() {
        bump(&watchers);
    }
}

pub fn is_online(user: u64) -> bool {
    SEEN.lock()
        .unwrap()
        .get(&user)
        .is_some_and(|at| at.elapsed() < ONLINE_FOR)
}

pub fn touch(user: u64) {
    let arrived = SEEN
        .lock()
        .unwrap()
        .insert(user, Instant::now())
        .is_none_or(|at| at.elapsed() >= ONLINE_FOR);

    if arrived {
        bump_watchers_of(user);
    }
}

pub fn sweep() {
    let gone: Vec<u64> = {
        let mut seen = SEEN.lock().unwrap();
        let gone: Vec<u64> = seen
            .iter()
            .filter(|(_, at)| at.elapsed() >= ONLINE_FOR)
            .map(|(user, _)| *user)
            .collect();

        for user in &gone {
            seen.remove(user);
        }

        gone
    };

    for user in gone {
        bump_watchers_of(user);
    }
}

pub fn spawn_sweeper() {
    tokio::spawn(async {
        let mut every = tokio::time::interval(Duration::from_secs(10));

        loop {
            every.tick().await;

            // The sweep reaches the database, which is synchronous.
            let _ = tokio::task::spawn_blocking(sweep).await;
        }
    });
}

pub async fn changed_since(user: u64, since: Option<u64>, wait: Duration) -> u64 {
    let Some(since) = since else {
        return revision(user);
    };

    let mut rx = TICK.subscribe();
    let deadline = tokio::time::Instant::now() + wait.min(MAX_WAIT);

    loop {
        let now = revision(user);

        if now != since {
            return now;
        }

        match tokio::time::timeout_at(deadline, rx.changed()).await {
            Ok(Ok(())) => continue,
            _ => return revision(user),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_stale_revision_returns_at_once() {
        let user = 0xA11CE;
        bump(&[user]);

        let started = Instant::now();
        let r = changed_since(user, Some(0), Duration::from_secs(5)).await;

        assert_ne!(r, 0);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[tokio::test]
    async fn no_revision_returns_at_once() {
        let started = Instant::now();
        changed_since(0xB0B, None, Duration::from_secs(5)).await;

        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[tokio::test]
    async fn a_bump_wakes_a_waiter() {
        let user = 0xCAFE;
        let since = revision(user);

        let waiter = tokio::spawn(changed_since(user, Some(since), Duration::from_secs(5)));

        tokio::time::sleep(Duration::from_millis(50)).await;
        bump(&[user]);

        let r = tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("the waiter to wake")
            .unwrap();

        assert_eq!(r, since + 1);
    }

    #[tokio::test]
    async fn an_unrelated_bump_does_not_end_the_wait() {
        let user = 0xD00D;
        let since = revision(user);

        let waiter = tokio::spawn(changed_since(user, Some(since), Duration::from_millis(300)));

        tokio::time::sleep(Duration::from_millis(50)).await;
        bump(&[0xE1E]);

        let started = Instant::now();
        let r = waiter.await.unwrap();

        assert_eq!(r, since);
        assert!(started.elapsed() >= Duration::from_millis(150));
    }
}
