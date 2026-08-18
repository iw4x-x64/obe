use log::info;
use rusqlite::Connection;
use std::cell::RefCell;
use std::fs::create_dir_all;

thread_local! {
    pub static SOCIAL_DB: RefCell<Connection> = RefCell::new(initialized_db());
}

fn initialized_db() -> Connection {
    create_dir_all("db").expect("to be able to create dir");

    let conn = Connection::open("db/social.db").expect("expected db connection to be able to open");

    let version: u64 = conn
        .query_row("PRAGMA user_version", (), |row| row.get(0))
        .expect("Version to be available");

    if version < 1 {
        conn.execute(
            "CREATE TABLE friend (
                    user_id INTEGER NOT NULL,
                    friend_id INTEGER NOT NULL,
                    created_at INTEGER NOT NULL,
                    PRIMARY KEY (user_id, friend_id)
                 )",
            (),
        )
        .expect("Initialization to succeed");

        conn.execute(
            "CREATE TABLE friend_request (
                    from_id INTEGER NOT NULL,
                    to_id INTEGER NOT NULL,
                    created_at INTEGER NOT NULL,
                    PRIMARY KEY (from_id, to_id)
                 )",
            (),
        )
        .expect("Initialization to succeed");

        conn.execute(
            "CREATE TABLE known_user (
                    user_id INTEGER PRIMARY KEY,
                    username TEXT NOT NULL,
                    seen_at INTEGER NOT NULL
                 )",
            (),
        )
        .expect("Initialization to succeed");

        conn.execute(
            "CREATE TABLE activity (
                    user_id INTEGER PRIMARY KEY,
                    connection TEXT NOT NULL,
                    updated_at INTEGER NOT NULL
                 )",
            (),
        )
        .expect("Initialization to succeed");

        conn.execute(
            "CREATE TABLE invite (
                    to_id INTEGER NOT NULL,
                    from_id INTEGER NOT NULL,
                    connection TEXT NOT NULL,
                    created_at INTEGER NOT NULL,
                    PRIMARY KEY (to_id, from_id)
                 )",
            (),
        )
        .expect("Initialization to succeed");

        conn.execute("PRAGMA user_version = 1", ())
            .expect("Setting pragma to succeed");

        info!("Initialized social db");
    }

    conn
}
