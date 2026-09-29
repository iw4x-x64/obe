use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, StatusCode, header};
use axum::routing::post;
use futures_util::StreamExt;
use log::{error, info, warn};
use rand::RngExt;
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::AsyncWriteExt;

const DIRECTORY: &str = "recordings";
const MAGIC: &[u8] = b"IW4XREC\0";
const MAX_BYTES: u64 = 256 << 20;

pub fn router() -> Router {
    Router::new().route("/v1/recordings", post(upload))
}

struct Metadata {
    xuid: u64,
    started: i64,
    map: String,
    gametype: String,
    client: String,
}

async fn upload(headers: HeaderMap, body: Body) -> StatusCode {
    let Some(metadata) = metadata(&headers) else {
        warn!("Recording upload with missing or malformed headers");
        return StatusCode::BAD_REQUEST;
    };

    if let Some(length) = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        && length > MAX_BYTES
    {
        warn!("Recording upload from {} of {length} bytes is too large", metadata.xuid);
        return StatusCode::PAYLOAD_TOO_LARGE;
    }

    if let Err(e) = fs::create_dir_all(DIRECTORY).await {
        error!("Failed to create the recordings directory: {e}");
        return StatusCode::INTERNAL_SERVER_ERROR;
    }

    let nonce: u32 = rand::rng().random();
    let name = format!(
        "{}-{}-{}-{}-{nonce:08x}.iw4rec",
        metadata.xuid, metadata.started, metadata.map, metadata.gametype
    );
    let file = PathBuf::from(DIRECTORY).join(&name);
    let part = file.with_extension("iw4rec.part");

    let size = match store(&part, body).await {
        Ok(size) => size,
        Err(status) => {
            let _ = fs::remove_file(&part).await;
            warn!("Recording upload from {} rejected with {status}", metadata.xuid);
            return status;
        }
    };

    if let Err(e) = fs::rename(&part, &file).await {
        error!("Failed to move recording {name} into place: {e}");
        let _ = fs::remove_file(&part).await;
        return StatusCode::INTERNAL_SERVER_ERROR;
    }

    info!(
        "Recording {name} saved from {} ({}): {} KB",
        metadata.xuid,
        metadata.client,
        size / 1024
    );

    StatusCode::CREATED
}

fn metadata(headers: &HeaderMap) -> Option<Metadata> {
    let value = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());

    let map = sanitize(value("x-iw4x-map")?);
    let gametype = sanitize(value("x-iw4x-gametype")?);

    if map.is_empty() || gametype.is_empty() {
        return None;
    }

    Some(Metadata {
        xuid: value("x-iw4x-xuid")?.parse().ok()?,
        started: value("x-iw4x-started")?.parse().ok()?,
        map,
        gametype,
        client: sanitize(value("x-iw4x-client").unwrap_or_default()),
    })
}

fn sanitize(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.'))
        .take(64)
        .collect()
}

async fn store(part: &Path, body: Body) -> Result<u64, StatusCode> {
    let internal = |e: std::io::Error| {
        error!("Failed to write recording {}: {e}", part.display());
        StatusCode::INTERNAL_SERVER_ERROR
    };

    let mut file = fs::File::create(part).await.map_err(internal)?;
    let mut stream = body.into_data_stream();
    let mut head = Vec::with_capacity(MAGIC.len());
    let mut size = 0u64;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| StatusCode::BAD_REQUEST)?;

        if head.len() < MAGIC.len() {
            let take = (MAGIC.len() - head.len()).min(chunk.len());
            head.extend_from_slice(&chunk[..take]);

            if !MAGIC.starts_with(&head) {
                return Err(StatusCode::UNPROCESSABLE_ENTITY);
            }
        }

        size += chunk.len() as u64;

        if size > MAX_BYTES {
            return Err(StatusCode::PAYLOAD_TOO_LARGE);
        }

        file.write_all(&chunk).await.map_err(internal)?;
    }

    if head.len() < MAGIC.len() {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }

    file.sync_all().await.map_err(internal)?;

    Ok(size)
}
