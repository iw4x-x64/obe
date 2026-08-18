use axum::Json;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::Response;
use futures_util::stream;
use std::convert::Infallible;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use bitdemon::auth::auth_proof::ClientOpaqueAuthProof;
use bitdemon::auth::key_store::ThreadSafeBackendPrivateKeyStorage;
use bitdemon::auth::result::auth_ticket::{AuthTicket, BdAuthTicketType};
use bitdemon::domain::title::Title;
use bitdemon::messaging::bd_serialization::BdSerialize;
use bitdemon::messaging::bd_writer::BdWriter;
use chrono::Utc;
use log::{info, warn};
use num_traits::FromPrimitive;
use rand::RngExt;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

const AUTH_TASK_REQUEST: u64 = 44;
const AUTH_TASK_REPLY: u64 = 45;

const AUTH_CODE_SUCCESS: u64 = 700;

const TICKET_SIZE: usize = 128;

const TICKET_LIFETIME_SECONDS: i64 = 5 * 60;

#[derive(Debug, Deserialize)]
pub struct AuthRequest {
    auth_task: String,
    title_id: String,
    iv_seed: String,
}

#[derive(Debug, Serialize)]
pub struct AuthReply {
    auth_task: u64,
    code: u64,
    iv_seed: u32,
    client_ticket: String,
    server_ticket: String,
    extra_data: String,
}

#[derive(Debug, Serialize)]
struct AuthError {
    auth_task: u64,
    code: u64,
    message: String,
}

#[derive(Clone)]
pub struct AuthState {
    pub key_store: Arc<ThreadSafeBackendPrivateKeyStorage>,
}

fn serialize_ticket(ticket: &AuthTicket) -> Result<[u8; TICKET_SIZE], Box<dyn std::error::Error>> {
    let mut buf = Vec::new();
    {
        let mut writer = BdWriter::new(&mut buf);
        ticket.serialize(&mut writer)?;
    }

    if buf.len() > TICKET_SIZE {
        return Err(format!("ticket serialized to {} bytes", buf.len()).into());
    }

    buf.resize(TICKET_SIZE, 0);

    let mut out = [0u8; TICKET_SIZE];
    out.copy_from_slice(&buf);
    Ok(out)
}

fn identify(headers: &HeaderMap) -> Option<(u64, String)> {
    let token = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let token = token.strip_prefix("IW4x1.0 ")?;

    let mut xuid = None;
    let mut gamertag = None;

    for field in token.split(';') {
        match field.split_once('=') {
            Some(("xuid", v)) => xuid = u64::from_str_radix(v, 16).ok(),
            Some(("gamertag", v)) if !v.is_empty() => gamertag = Some(v.to_string()),
            _ => {}
        }
    }

    Some((xuid?, gamertag?))
}

pub async fn handle_auth(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Json(request): Json<AuthRequest>,
) -> Response {
    let task: u64 = request.auth_task.parse().unwrap_or(0);
    let title_id: u32 = request.title_id.parse().unwrap_or(0);
    let iv_seed: u32 = request.iv_seed.parse().unwrap_or(0);

    info!("Auth request task={task} title={title_id} iv_seed={iv_seed}");

    if task != AUTH_TASK_REQUEST {
        warn!("Rejecting unknown auth task {task}");
        return error(AUTH_TASK_REPLY, 1, format!("unsupported auth task {task}"));
    }

    let Some(title) = Title::from_u32(title_id) else {
        warn!("Rejecting unknown title {title_id}");
        return error(AUTH_TASK_REPLY, 1, format!("unknown title {title_id}"));
    };

    let (user_id, username) = identify(&headers).unwrap_or_else(|| {
        warn!("No IW4x identity on the auth request; falling back to a fixed one");
        (1, String::from("player"))
    });

    crate::social::note_user(user_id, username.as_str());

    let now = Utc::now();
    let issued = (now.timestamp() % (u32::MAX as i64)) as u32;
    let expires_i64 = now.timestamp() + TICKET_LIFETIME_SECONDS;
    let expires = (expires_i64 % (u32::MAX as i64)) as u32;

    let session_key: [u8; 24] = rand::rng().random();

    let ticket = AuthTicket {
        ticket_type: BdAuthTicketType::UserToService,
        title,
        time_issued: issued,
        time_expires: expires,
        license_id: 1,
        user_id,
        username: username.clone(),
        session_key,
    };

    let client_ticket = match serialize_ticket(&ticket) {
        Ok(t) => t,
        Err(e) => {
            warn!("Failed to serialize client ticket: {e}");
            return error(AUTH_TASK_REPLY, 1, String::from("internal error"));
        }
    };

    let proof = ClientOpaqueAuthProof {
        title,
        time_expires: expires_i64,
        license_id: ticket.license_id,
        user_id,
        session_key,
        username,
    };

    let server_ticket = proof.serialize(state.key_store.as_ref());

    log::trace!("session_key={session_key:02x?}");
    log::trace!("server_ticket={:02x?}", &server_ticket[..]);

    let extra_data = serde_json::json!({ "sbx": "RETAIL", "dpi": "0" }).to_string();

    let reply = AuthReply {
        auth_task: AUTH_TASK_REPLY,
        code: AUTH_CODE_SUCCESS,
        iv_seed,
        client_ticket: BASE64.encode(client_ticket),
        server_ticket: BASE64.encode(server_ticket),
        extra_data,
    };

    info!("Issued ticket for title={title:?} user={user_id} expires={expires}");

    reply_json(serde_json::to_value(reply).unwrap())
}

fn error(task: u64, code: u64, message: String) -> Response {
    reply_json(
        serde_json::to_value(AuthError {
            auth_task: task,
            code,
            message,
        })
        .unwrap(),
    )
}

fn reply_json(value: serde_json::Value) -> Response {
    let bytes = serde_json::to_vec(&value).unwrap();
    let len = bytes.len();

    let body = stream::once(async move {
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        Ok::<_, Infallible>(Bytes::from(bytes))
    });

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CONTENT_LENGTH, len)
        .body(Body::from_stream(body))
        .unwrap()
}
