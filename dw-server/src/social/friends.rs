use crate::social::db::SOCIAL_DB;
use chrono::Utc;
use log::info;
use rusqlite::params;

pub struct Friendship {
    pub user_id: u64,
    pub username: String,

    pub mutual: bool,
}

pub fn note_user(user_id: u64, username: &str) {
    let now = Utc::now().timestamp();

    SOCIAL_DB.with_borrow(|conn| {
        let _ = conn.execute(
            "INSERT INTO known_user (user_id, username, seen_at) VALUES (?1, ?2, ?3)
             ON CONFLICT (user_id) DO UPDATE SET username = ?2, seen_at = ?3",
            params![user_id, username, now],
        );
    });
}

#[allow(dead_code)]
pub fn lookup_name(user_id: u64) -> Option<String> {
    SOCIAL_DB.with_borrow(|conn| {
        conn.query_row(
            "SELECT username FROM known_user WHERE user_id = ?1",
            params![user_id],
            |row| row.get(0),
        )
        .ok()
    })
}

pub fn lookup_ids(username: &str) -> Vec<u64> {
    SOCIAL_DB.with_borrow(|conn| {
        let mut statement = match conn.prepare(
            "SELECT user_id FROM known_user
              WHERE username = ?1 COLLATE NOCASE
              ORDER BY seen_at DESC",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        let rows = statement.query_map(params![username], |row| row.get(0));

        match rows {
            Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
            Err(_) => Vec::new(),
        }
    })
}

pub struct Match {
    pub user_id: u64,
    pub username: String,
}

const SEARCH_LIMIT: i64 = 64;

pub fn like_pattern(query: &str) -> String {
    let mut out = String::with_capacity(query.len() + 2);

    out.push('%');

    for c in query.chars() {
        if matches!(c, '\\' | '%' | '_') {
            out.push('\\');
        }

        out.push(c);
    }

    out.push('%');
    out
}

pub fn search(query: &str) -> Vec<Match> {
    if query.trim().is_empty() {
        return Vec::new();
    }

    SOCIAL_DB.with_borrow(|conn| {
        let mut statement = match conn.prepare(
            "SELECT user_id, username
               FROM known_user
              WHERE username LIKE ?1 ESCAPE '\\'
              ORDER BY (username = ?2 COLLATE NOCASE) DESC, seen_at DESC
              LIMIT ?3",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        let rows = statement.query_map(
            params![like_pattern(query), query, SEARCH_LIMIT],
            |row| {
                Ok(Match {
                    user_id: row.get(0)?,
                    username: row.get(1)?,
                })
            },
        );

        match rows {
            Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
            Err(_) => Vec::new(),
        }
    })
}

pub fn list(user_id: u64) -> Vec<Friendship> {
    SOCIAL_DB.with_borrow(|conn| {
        let mut statement = match conn.prepare(
            "SELECT f.friend_id,
                    COALESCE(k.username, ''),
                    EXISTS (SELECT 1 FROM friend r
                             WHERE r.user_id = f.friend_id AND r.friend_id = f.user_id)
               FROM friend f
               LEFT JOIN known_user k ON k.user_id = f.friend_id
              WHERE f.user_id = ?1
              ORDER BY f.created_at",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        let rows = statement.query_map(params![user_id], |row| {
            Ok(Friendship {
                user_id: row.get(0)?,
                username: row.get(1)?,
                mutual: row.get(2)?,
            })
        });

        match rows {
            Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
            Err(_) => Vec::new(),
        }
    })
}

pub fn incoming_requests(user_id: u64) -> Vec<Friendship> {
    SOCIAL_DB.with_borrow(|conn| {
        let mut statement = match conn.prepare(
            "SELECT r.from_id, COALESCE(k.username, '')
               FROM friend_request r
               LEFT JOIN known_user k ON k.user_id = r.from_id
              WHERE r.to_id = ?1
              ORDER BY r.created_at",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        let rows = statement.query_map(params![user_id], |row| {
            Ok(Friendship {
                user_id: row.get(0)?,
                username: row.get(1)?,
                mutual: false,
            })
        });

        match rows {
            Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
            Err(_) => Vec::new(),
        }
    })
}

pub fn outgoing_requests(user_id: u64) -> Vec<Friendship> {
    SOCIAL_DB.with_borrow(|conn| {
        let mut statement = match conn.prepare(
            "SELECT r.to_id, COALESCE(k.username, '')
               FROM friend_request r
               LEFT JOIN known_user k ON k.user_id = r.to_id
              WHERE r.from_id = ?1
              ORDER BY r.created_at",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        let rows = statement.query_map(params![user_id], |row| {
            Ok(Friendship {
                user_id: row.get(0)?,
                username: row.get(1)?,
                mutual: false,
            })
        });

        match rows {
            Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
            Err(_) => Vec::new(),
        }
    })
}

pub fn request(from: u64, to: u64) -> Result<&'static str, &'static str> {
    if from == to {
        return Err("cannot befriend yourself");
    }

    let now = Utc::now().timestamp();

    SOCIAL_DB.with_borrow(|conn| {
        let pending: bool = conn
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM friend_request WHERE from_id = ?1 AND to_id = ?2)",
                params![to, from],
                |row| row.get(0),
            )
            .unwrap_or(false);

        if pending {
            return accept_locked(conn, from, to, now);
        }

        match conn.execute(
            "INSERT OR IGNORE INTO friend_request (from_id, to_id, created_at)
             VALUES (?1, ?2, ?3)",
            params![from, to, now],
        ) {
            Ok(_) => {
                info!("Friend request from {from} to {to}");
                Ok("requested")
            }
            Err(_) => Err("could not record the request"),
        }
    })
}

pub fn accept(user: u64, from: u64) -> Result<&'static str, &'static str> {
    let now = Utc::now().timestamp();

    SOCIAL_DB.with_borrow(|conn| {
        let pending: bool = conn
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM friend_request WHERE from_id = ?1 AND to_id = ?2)",
                params![from, user],
                |row| row.get(0),
            )
            .unwrap_or(false);

        if !pending {
            return Err("no such request");
        }

        accept_locked(conn, user, from, now)
    })
}

fn accept_locked(
    conn: &rusqlite::Connection,
    user: u64,
    other: u64,
    now: i64,
) -> Result<&'static str, &'static str> {
    let _ = conn.execute(
        "DELETE FROM friend_request WHERE (from_id = ?1 AND to_id = ?2)
                                       OR (from_id = ?2 AND to_id = ?1)",
        params![user, other],
    );

    for (a, b) in [(user, other), (other, user)] {
        if conn
            .execute(
                "INSERT OR IGNORE INTO friend (user_id, friend_id, created_at)
                 VALUES (?1, ?2, ?3)",
                params![a, b, now],
            )
            .is_err()
        {
            return Err("could not record the friendship");
        }
    }

    info!("{user} and {other} are now friends");
    Ok("friends")
}

pub fn remove(user: u64, other: u64) -> Result<&'static str, &'static str> {
    SOCIAL_DB.with_borrow(|conn| {
        let _ = conn.execute(
            "DELETE FROM friend WHERE (user_id = ?1 AND friend_id = ?2)
                                   OR (user_id = ?2 AND friend_id = ?1)",
            params![user, other],
        );
        let _ = conn.execute(
            "DELETE FROM friend_request WHERE (from_id = ?1 AND to_id = ?2)
                                           OR (from_id = ?2 AND to_id = ?1)",
            params![user, other],
        );

        Ok("removed")
    })
}
