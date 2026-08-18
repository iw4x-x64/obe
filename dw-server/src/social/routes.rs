use crate::social::activity;
use crate::social::friends;
use axum::Router;
use axum::extract::{Path, Query};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, http::{HeaderMap, StatusCode, header}};
use log::{info, trace, warn};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Serialize)]
struct Person {
    xuid: String,

    #[serde(rename = "isFavorite")]
    is_favorite: bool,

    #[serde(rename = "isFollowingCaller")]
    is_following_caller: bool,

    #[serde(rename = "socialNetworks")]
    social_networks: Vec<String>,
}

#[derive(Serialize)]
struct PeopleResponse {
    #[serde(rename = "totalCount")]
    total_count: usize,
    people: Vec<Person>,
}

async fn people(Path(user): Path<String>, Query(q): Query<HashMap<String, String>>) -> Response {
    let Some(user_id) = parse_xuid(user.as_str()) else {
        warn!("Social request for an unparseable user '{user}'");
        return (StatusCode::BAD_REQUEST, "bad user").into_response();
    };

    let friends = friends::list(user_id);

    info!(
        "Social: {} friends for {user_id} (view {})",
        friends.len(),
        q.get("view").map(String::as_str).unwrap_or("-")
    );

    let people: Vec<Person> = friends
        .iter()
        .map(|f| Person {
            xuid: f.user_id.to_string(),
            is_favorite: false,
            is_following_caller: f.mutual,
            social_networks: Vec::new(),
        })
        .collect();

    Json(PeopleResponse {
        total_count: people.len(),
        people,
    })
    .into_response()
}

fn parse_xuid(segment: &str) -> Option<u64> {
    segment
        .strip_prefix("xuid(")
        .and_then(|s| s.strip_suffix(')'))
        .and_then(|s| s.parse().ok())
        .or_else(|| segment.parse().ok())
}


#[derive(Deserialize)]
struct FriendAction {
    user: String,
    other: String,
}

#[derive(Serialize)]
struct ActionResult {
    result: String,
}

fn resolve(who: &str) -> Option<u64> {
    who.parse().ok().or_else(|| friends::lookup_id(who))
}

fn act(
    body: FriendAction,
    f: impl Fn(u64, u64) -> Result<&'static str, &'static str>,
) -> Response {
    let (Some(user), Some(other)) = (resolve(&body.user), resolve(&body.other)) else {
        return (
            StatusCode::NOT_FOUND,
            Json(ActionResult {
                result: "unknown player; they must have signed in at least once".to_string(),
            }),
        )
            .into_response();
    };

    match f(user, other) {
        Ok(result) => Json(ActionResult {
            result: result.to_string(),
        })
        .into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(ActionResult {
                result: e.to_string(),
            }),
        )
            .into_response(),
    }
}

async fn request(Json(body): Json<FriendAction>) -> Response {
    act(body, friends::request)
}

async fn accept(Json(body): Json<FriendAction>) -> Response {
    act(body, friends::accept)
}

async fn remove(Json(body): Json<FriendAction>) -> Response {
    act(body, friends::remove)
}

#[derive(Serialize)]
struct FriendView {
    id: u64,
    name: String,
    mutual: bool,
}

async fn list(Path(who): Path<String>) -> Response {
    let Some(user) = resolve(who.as_str()) else {
        return (StatusCode::NOT_FOUND, "unknown player").into_response();
    };

    let view = |f: &friends::Friendship| FriendView {
        id: f.user_id,
        name: f.username.clone(),
        mutual: f.mutual,
    };

    Json(serde_json::json!({
        "friends": friends::list(user).iter().map(view).collect::<Vec<_>>(),
        "incoming": friends::incoming_requests(user).iter().map(view).collect::<Vec<_>>(),
    }))
    .into_response()
}


fn xuid_in(path: &str) -> Option<u64> {
    if let Some(at) = path.find("xuid(") {
        let rest = &path[at + 5..];
        let end = rest.find(')')?;
        return rest[..end].parse().ok();
    }

    path.parse().ok()
}

async fn set_activity(Path((title, user)): Path<(String, String)>, body: String) -> Response {
    trace!("Activity for {user} in title {title}: {body}");

    let Some(user_id) = xuid_in(user.as_str()) else {
        return (StatusCode::BAD_REQUEST, "bad user").into_response();
    };

    if let Some(connection) = member(body.as_str(), "connectionString") {
        activity::set_activity(user_id, connection.as_str());
    } else {
        warn!("Activity for {user_id} carries no connectionString: {body}");
    }

    StatusCode::OK.into_response()
}

async fn delete_activity(Path((_title, user)): Path<(String, String)>) -> Response {
    if let Some(user_id) = xuid_in(user.as_str()) {
        activity::clear_activity(user_id);
    }

    StatusCode::OK.into_response()
}

async fn send_invites(headers: HeaderMap, Path(title): Path<String>, body: String) -> Response {
    trace!("Invites in title {title}: {body}");

    let Some(from) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|t| t.strip_prefix("IW4x1.0 "))
        .and_then(|t| {
            t.split(';')
                .find_map(|f| f.strip_prefix("xuid="))
                .and_then(|v| u64::from_str_radix(v, 16).ok())
        })
    else {
        warn!("Invite with no IW4x identity on it");
        return (StatusCode::UNAUTHORIZED, "no identity").into_response();
    };

    let connection = match member(body.as_str(), "connectionString") {
        Some(c) => c,
        None => match activity::activity_of(from) {
            Some(c) => c,
            None => {
                warn!("Invite from {from} carries no session: {body}");
                return StatusCode::OK.into_response();
            }
        },
    };

    let mut sent = 0;

    for to in xuids_in(body.as_str()) {
        activity::invite(from, to, connection.as_str());
        sent += 1;
    }

    if sent == 0 {
        warn!("Invite from {from} named nobody this side understands: {body}");
    }

    StatusCode::OK.into_response()
}

fn xuids_in(body: &str) -> Vec<u64> {
    let Some(at) = body.find("\"invitedUsers\"") else {
        return Vec::new();
    };

    let rest = &body[at..];

    let Some(open) = rest.find('[') else {
        return Vec::new();
    };

    let Some(close) = rest[open..].find(']') else {
        return Vec::new();
    };

    rest[open + 1..open + close]
        .split(',')
        .filter_map(|f| f.trim().trim_matches('"').parse().ok())
        .collect()
}

fn member(body: &str, key: &str) -> Option<String> {
    let at = body.find(&format!("\"{key}\""))?;
    let rest = &body[at + key.len() + 2..];
    let colon = rest.find(':')?;
    let rest = &rest[colon + 1..];
    let open = rest.find('"')?;
    let rest = &rest[open + 1..];

    let mut out = String::new();
    let mut chars = rest.chars();

    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => {
                if let Some(n) = chars.next() {
                    out.push(n);
                }
            }
            _ => out.push(c),
        }
    }

    None
}

#[derive(Serialize)]
struct InviteView {
    from: u64,
    name: String,
    connection: String,
}

async fn invites(Path(who): Path<String>) -> Response {
    let Some(user) = resolve(who.as_str()) else {
        return (StatusCode::NOT_FOUND, "unknown player").into_response();
    };

    let waiting: Vec<InviteView> = activity::take_invites(user)
        .into_iter()
        .map(|i| InviteView {
            from: i.from,
            name: i.from_name,
            connection: i.connection,
        })
        .collect();

    Json(serde_json::json!({ "invites": waiting })).into_response()
}

pub fn router() -> Router {
    Router::new()
        .route("/users/{user}/people", get(people))
        .route("/iw4x/friends/{who}", get(list))
        .route("/iw4x/friends/request", post(request))
        .route("/iw4x/friends/accept", post(accept))
        .route("/iw4x/friends/remove", post(remove))
        .route("/iw4x/invites/{who}", get(invites))
        .route(
            "/titles/{title}/users/{user}/activities",
            put(set_activity).post(set_activity).delete(delete_activity),
        )
        .route("/titles/{title}/invites", post(send_invites))
}
