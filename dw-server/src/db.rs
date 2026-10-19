use rusqlite::Connection;
use std::sync::{Mutex, MutexGuard, OnceLock};

/// A database the whole server shares.
///
/// Every connection the server accepts gets an operating system thread of its
/// own, so a connection per thread means the open files grow with the number of
/// players and eventually run the process out of descriptors.
pub struct Db {
    open: fn() -> Connection,
    connection: OnceLock<Mutex<Connection>>,
}

impl Db {
    pub const fn new(open: fn() -> Connection) -> Db {
        Db {
            open,
            connection: OnceLock::new(),
        }
    }

    /// Opens the database if it is not open yet, so that a server that cannot
    /// reach its files says so at startup rather than under the first player.
    pub fn open(&self) {
        drop(self.lock());
    }

    pub fn with_borrow<R>(&self, f: impl FnOnce(&Connection) -> R) -> R {
        f(&self.lock())
    }

    pub fn with_borrow_mut<R>(&self, f: impl FnOnce(&mut Connection) -> R) -> R {
        f(&mut self.lock())
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        self.connection
            .get_or_init(|| Mutex::new((self.open)()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;

    static COUNTED: Db = Db::new(counted);

    static OPENED: Mutex<usize> = Mutex::new(0);

    fn counted() -> Connection {
        *OPENED.lock().unwrap() += 1;

        Connection::open_in_memory().unwrap()
    }

    #[test]
    fn every_thread_shares_one_connection() {
        let threads = 8;
        let ready = Barrier::new(threads);

        std::thread::scope(|scope| {
            for _ in 0..threads {
                scope.spawn(|| {
                    ready.wait();
                    COUNTED.with_borrow(|conn| {
                        conn.query_row("SELECT 1", (), |row| row.get::<_, i64>(0))
                            .unwrap()
                    });
                });
            }
        });

        assert_eq!(*OPENED.lock().unwrap(), 1);
    }

    #[test]
    fn a_panicking_query_does_not_close_the_database() {
        let db = Db::new(|| Connection::open_in_memory().unwrap());

        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            db.with_borrow(|_| panic!("a query gave up"));
        }));

        assert!(panicked.is_err());
        assert_eq!(
            db.with_borrow(|conn| conn
                .query_row("SELECT 7", (), |row| row.get::<_, i64>(0))
                .unwrap()),
            7
        );
    }
}
