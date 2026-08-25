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

    #[serde(default)]
    id: Option<String>,

    #[serde(default)]
    other: Option<String>,
}

#[derive(Serialize)]
struct ActionResult {
    result: String,
}

enum Found {
    One(u64),
    Many(Vec<u64>),
    None,
}

fn by_name(who: &str) -> Found {
    let ids = friends::lookup_ids(who);

    match ids.len() {
        0 => Found::None,
        1 => Found::One(ids[0]),
        _ => Found::Many(ids),
    }
}

fn by_id(what: &str) -> Result<u64, NoOne> {
    what.parse::<u64>()
        .map_err(|_| NoOne::NotAnId(what.to_string()))
}

fn caller(who: &str) -> Found {
    if let Ok(id) = who.parse::<u64>() {
        return Found::One(id);
    }

    by_name(who)
}

fn resolve(who: &str) -> Option<u64> {
    match caller(who) {
        Found::One(id) => Some(id),
        Found::Many(ids) => {
            warn!(
                "{} players answer to '{who}'; taking the one seen most recently",
                ids.len()
            );

            ids.first().copied()
        }
        Found::None => None,
    }
}

#[derive(Debug, PartialEq, Eq)]
enum NoOne {
    Missing,
    NotAnId(String),
    Ambiguous(String, usize),
    Unknown,
}

fn refused(why: NoOne) -> Response {
    let (status, result) = match why {
        NoOne::Missing => (
            StatusCode::BAD_REQUEST,
            "the request names nobody".to_string(),
        ),
        NoOne::NotAnId(what) => (
            StatusCode::BAD_REQUEST,
            format!("\"{what}\" is not a player id"),
        ),
        NoOne::Ambiguous(who, how_many) => (
            StatusCode::CONFLICT,
            format!("{how_many} players answer to \"{who}\"; search and pick one"),
        ),
        NoOne::Unknown => (
            StatusCode::NOT_FOUND,
            "unknown player; they must have signed in at least once".to_string(),
        ),
    };

    (status, Json(ActionResult { result })).into_response()
}

#[derive(Clone, Copy)]
enum Other {
    Name,
    Id,
}

fn target(body: &FriendAction, other_is: Other) -> Result<u64, NoOne> {
    if let Some(id) = body.id.as_deref() {
        return by_id(id);
    }

    let Some(other) = body.other.as_deref() else {
        return Err(NoOne::Missing);
    };

    match other_is {
        Other::Id => by_id(other),
        Other::Name => match by_name(other) {
            Found::One(id) => Ok(id),
            Found::Many(ids) => Err(NoOne::Ambiguous(other.to_string(), ids.len())),
            Found::None => Err(NoOne::Unknown),
        },
    }
}

fn act(
    body: FriendAction,
    other_is: Other,
    f: impl Fn(u64, u64) -> Result<&'static str, &'static str>,
) -> Response {
    let user = match caller(&body.user) {
        Found::One(id) => id,
        Found::Many(ids) => {
            return refused(NoOne::Ambiguous(body.user.clone(), ids.len()));
        }
        Found::None => return refused(NoOne::Unknown),
    };

    let other = match target(&body, other_is) {
        Ok(id) => id,
        Err(why) => return refused(why),
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
    act(body, Other::Name, friends::request)
}

async fn accept(Json(body): Json<FriendAction>) -> Response {
    act(body, Other::Id, friends::accept)
}

async fn remove(Json(body): Json<FriendAction>) -> Response {
    act(body, Other::Id, friends::remove)
}

#[derive(Deserialize)]
struct SearchRequest {
    #[allow(dead_code)]
    user: String,
    name: String,
}

#[derive(Serialize)]
struct MatchView {
    id: u64,
    name: String,
}

async fn search(Json(body): Json<SearchRequest>) -> Response {
    let found = friends::search(body.name.as_str());

    info!("Social: {} players answer to '{}'", found.len(), body.name);

    let results: Vec<MatchView> = found
        .into_iter()
        .map(|m| MatchView {
            id: m.user_id,
            name: m.username,
        })
        .collect();

    Json(serde_json::json!({ "results": results })).into_response()
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

    let mut roster: Vec<FriendView> = friends::list(user).iter().map(view).collect();
    roster.extend(friends::outgoing_requests(user).iter().map(view));

    Json(serde_json::json!({
        "friends": roster,
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
        if let Err(what) = usable_session(connection.as_str()) {
            warn!("Activity for {user_id} {what}; not published: {connection}");
            return StatusCode::OK.into_response();
        }

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

    if let Err(what) = usable_session(connection.as_str()) {
        warn!("Invite from {from} {what}; not delivered: {connection}");
        return StatusCode::OK.into_response();
    }

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

fn usable_session(connection: &str) -> Result<(), &'static str> {
    if let Some(id) = member(connection, "dw_sec_kid")
        && repeated_byte(id.as_str())
    {
        return Err("carries a placeholder security ID");
    }

    if let Some(key) = member(connection, "dw_sec_key")
        && repeated_byte(key.as_str())
    {
        return Err("carries a placeholder security key");
    }

    Ok(())
}

fn repeated_byte(hex: &str) -> bool {
    let b = hex.as_bytes();

    !b.is_empty() && b.len().is_multiple_of(2) && b.chunks(2).all(|c| c == &b[..2])
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
        .route("/iw4x/friends/search", post(search))
        .route("/iw4x/friends/accept", post(accept))
        .route("/iw4x/friends/remove", post(remove))
        .route("/iw4x/invites/{who}", get(invites))
        .route(
            "/titles/{title}/users/{user}/activities",
            put(set_activity).post(set_activity).delete(delete_activity),
        )
        .route("/titles/{title}/invites", post(send_invites))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_placeholder_identity_is_refused() {
        let zeros = r#"{"dw_sec_kid":"0000000000000000","dw_sec_key":"01010101010101010101010101010101"}"#;
        assert!(usable_session(zeros).is_err());

        let ones = r#"{"dw_sec_kid":"2406ec84b2a3fea2","dw_sec_key":"01010101010101010101010101010101"}"#;
        assert!(usable_session(ones).is_err());
    }

    #[test]
    fn a_real_identity_is_kept() {
        let real = r#"{"dw_sec_kid":"2406ec84b2a3fea2","dw_sec_key":"c7f12bd8711bedc5f061feb4fd68f006"}"#;
        assert!(usable_session(real).is_ok());
    }

    #[test]
    fn a_string_of_another_shape_is_left_alone() {
        assert!(usable_session(r#"{"something":"else"}"#).is_ok());
    }

    fn action(id: Option<&str>, other: Option<&str>) -> FriendAction {
        FriendAction {
            user: "1".to_string(),
            id: id.map(str::to_string),
            other: other.map(str::to_string),
        }
    }

    #[test]
    fn an_id_names_the_player_it_names() {
        assert_eq!(
            target(&action(Some("2533274790395904"), None), Other::Name),
            Ok(2533274790395904)
        );
    }

    #[test]
    fn an_id_is_taken_over_a_name_beside_it() {
        assert_eq!(target(&action(Some("7"), Some("Soap")), Other::Name), Ok(7));
    }

    #[test]
    fn an_id_that_is_not_one_is_refused() {
        assert_eq!(
            target(&action(Some("Soap"), None), Other::Name),
            Err(NoOne::NotAnId("Soap".to_string()))
        );
    }

    #[test]
    fn a_request_naming_nobody_is_refused() {
        assert_eq!(target(&action(None, None), Other::Name), Err(NoOne::Missing));
    }

    #[test]
    fn an_endpoint_dealing_in_ids_reads_other_as_one() {
        assert_eq!(target(&action(None, Some("1337")), Other::Id), Ok(1337));
    }

    #[test]
    fn an_endpoint_dealing_in_ids_refuses_a_name() {
        assert_eq!(
            target(&action(None, Some("Soap")), Other::Id),
            Err(NoOne::NotAnId("Soap".to_string()))
        );
    }

    #[test]
    fn a_caller_that_reads_as_an_id_is_one() {
        assert!(matches!(caller("2533274790395904"), Found::One(2533274790395904)));
    }

    #[test]
    fn a_refusal_says_which_name_was_ambiguous() {
        let response = refused(NoOne::Ambiguous("Soap".to_string(), 3));
        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    #[test]
    fn a_search_pattern_matches_anywhere_in_a_name() {
        assert_eq!(friends::like_pattern("Soap"), "%Soap%");
    }

    #[test]
    fn a_wildcard_in_a_name_is_a_character_like_any_other() {
        assert_eq!(friends::like_pattern("100%"), "%100\\%%");
        assert_eq!(friends::like_pattern("a_b"), "%a\\_b%");
        assert_eq!(friends::like_pattern("a\\b"), "%a\\\\b%");
    }
}
