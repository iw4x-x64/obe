use crate::social::db::SOCIAL_DB;
use crate::social::watch;
use chrono::Utc;
use log::{info, trace};
use rusqlite::params;

pub fn set_activity(user_id: u64, connection: &str) {
    let now = Utc::now().timestamp();
    let changed = activity_of(user_id).is_none_or(|was| was != connection);

    SOCIAL_DB.with_borrow(|conn| {
        let _ = conn.execute(
            "INSERT INTO activity (user_id, connection, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT (user_id) DO UPDATE SET connection = ?2, updated_at = ?3",
            params![user_id, connection, now],
        );
    });

    info!("Activity for {user_id}: {connection}");

    if changed {
        watch::bump_watchers_of(user_id);
    }
}

pub fn clear_activity(user_id: u64) {
    let cleared = SOCIAL_DB.with_borrow(|conn| {
        conn.execute("DELETE FROM activity WHERE user_id = ?1", params![user_id])
            .unwrap_or(0)
    });

    if cleared != 0 {
        watch::bump_watchers_of(user_id);
    }

    trace!("Activity cleared for {user_id}");
}

pub fn activity_of(user_id: u64) -> Option<String> {
    SOCIAL_DB.with_borrow(|conn| {
        conn.query_row(
            "SELECT connection FROM activity WHERE user_id = ?1",
            params![user_id],
            |row| row.get(0),
        )
        .ok()
    })
}

pub fn invite(from: u64, to: u64, connection: &str) {
    let now = Utc::now().timestamp();

    SOCIAL_DB.with_borrow(|conn| {
        let _ = conn.execute(
            "INSERT INTO invite (to_id, from_id, connection, created_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (to_id, from_id) DO UPDATE SET connection = ?3, created_at = ?4",
            params![to, from, connection, now],
        );
    });

    info!("Invite from {from} to {to}: {connection}");

    watch::bump(&[to]);
}

pub struct Invitation {
    pub from: u64,
    pub from_name: String,
    pub connection: String,
}

pub fn take_invites(user_id: u64) -> Vec<Invitation> {
    SOCIAL_DB.with_borrow(|conn| {
        let mut statement = match conn.prepare(
            "SELECT i.from_id, COALESCE(k.username, ''), i.connection
               FROM invite i
               LEFT JOIN known_user k ON k.user_id = i.from_id
              WHERE i.to_id = ?1
              ORDER BY i.created_at",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        let rows = statement.query_map(params![user_id], |row| {
            Ok(Invitation {
                from: row.get(0)?,
                from_name: row.get(1)?,
                connection: row.get(2)?,
            })
        });

        let out: Vec<Invitation> = match rows {
            Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
            Err(_) => Vec::new(),
        };

        if !out.is_empty() {
            let _ = conn.execute("DELETE FROM invite WHERE to_id = ?1", params![user_id]);
        }

        out
    })
}
