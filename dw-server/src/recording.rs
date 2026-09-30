use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, StatusCode, header};
use axum::routing::post;
use futures_util::StreamExt;
use log::{error, info, warn};
use rand::RngExt;
use std::io::Write;
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::AsyncWriteExt;
use zstd::stream::raw::{Decoder, Operation};
use zstd::stream::write::Encoder;

const DIRECTORY: &str = "recordings";
const MAGIC: &[u8] = b"IW4XREC\0";
const MAX_BYTES: u64 = 512 << 20;
const LEVEL: i32 = 3;

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

enum Encoding {
    Identity,
    Zstd,
}

async fn upload(headers: HeaderMap, body: Body) -> StatusCode {
    let Some(metadata) = metadata(&headers) else {
        warn!("Recording upload with missing or malformed headers");
        return StatusCode::BAD_REQUEST;
    };

    let encoding = match headers
        .get(header::CONTENT_ENCODING)
        .map(|v| v.to_str().unwrap_or_default())
    {
        None | Some("identity") => Encoding::Identity,
        Some("zstd") => Encoding::Zstd,
        Some(other) => {
            warn!("Recording upload from {} in unsupported encoding {other:?}", metadata.xuid);
            return StatusCode::UNSUPPORTED_MEDIA_TYPE;
        }
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
        "{}-{}-{}-{}-{nonce:08x}.iw4rec.zst",
        metadata.xuid, metadata.started, metadata.map, metadata.gametype
    );
    let file = PathBuf::from(DIRECTORY).join(&name);
    let part = file.with_extension("zst.part");

    let size = match store(&part, body, encoding).await {
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

async fn store(part: &Path, body: Body, encoding: Encoding) -> Result<u64, StatusCode> {
    let internal = |e: std::io::Error| {
        error!("Failed to write recording {}: {e}", part.display());
        StatusCode::INTERNAL_SERVER_ERROR
    };

    let mut file = fs::File::create(part).await.map_err(internal)?;
    let mut stream = body.into_data_stream();
    let mut head = Head::new(&encoding).map_err(internal)?;
    let mut encoder = match encoding {
        Encoding::Identity => Some(Encoder::new(Vec::new(), LEVEL).map_err(internal)?),
        Encoding::Zstd => None,
    };
    let mut received = 0u64;
    let mut stored = 0u64;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| StatusCode::BAD_REQUEST)?;

        received += chunk.len() as u64;

        if received > MAX_BYTES {
            return Err(StatusCode::PAYLOAD_TOO_LARGE);
        }

        head.feed(&chunk)?;

        let out = match encoder.as_mut() {
            Some(encoder) => {
                encoder.write_all(&chunk).map_err(internal)?;
                std::mem::take(encoder.get_mut())
            }
            None => chunk.to_vec(),
        };

        stored += out.len() as u64;
        file.write_all(&out).await.map_err(internal)?;
    }

    if !head.complete() {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }

    if let Some(encoder) = encoder {
        let out = encoder.finish().map_err(internal)?;
        stored += out.len() as u64;
        file.write_all(&out).await.map_err(internal)?;
    }

    file.sync_all().await.map_err(internal)?;

    Ok(stored)
}

struct Head {
    bytes: Vec<u8>,
    decoder: Option<Decoder<'static>>,
}

impl Head {
    fn new(encoding: &Encoding) -> std::io::Result<Head> {
        Ok(Head {
            bytes: Vec::with_capacity(MAGIC.len()),
            decoder: match encoding {
                Encoding::Identity => None,
                Encoding::Zstd => Some(Decoder::new()?),
            },
        })
    }

    fn complete(&self) -> bool {
        self.bytes.len() == MAGIC.len()
    }

    fn feed(&mut self, mut chunk: &[u8]) -> Result<(), StatusCode> {
        while !self.complete() && !chunk.is_empty() {
            let mut out = [0u8; MAGIC.len()];
            let want = MAGIC.len() - self.bytes.len();

            let (read, written) = match self.decoder.as_mut() {
                Some(decoder) => {
                    let status = decoder
                        .run_on_buffers(chunk, &mut out[..want])
                        .map_err(|_| StatusCode::UNPROCESSABLE_ENTITY)?;

                    (status.bytes_read, status.bytes_written)
                }
                None => {
                    let n = want.min(chunk.len());
                    out[..n].copy_from_slice(&chunk[..n]);
                    (n, n)
                }
            };

            if read == 0 && written == 0 {
                break;
            }

            self.bytes.extend_from_slice(&out[..written]);
            chunk = &chunk[read..];

            if !MAGIC.starts_with(&self.bytes) {
                return Err(StatusCode::UNPROCESSABLE_ENTITY);
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recording() -> Vec<u8> {
        let mut r = MAGIC.to_vec();
        r.extend((0..200_000u32).flat_map(|i| (i % 97).to_le_bytes()));
        r
    }

    async fn stored(body: Vec<u8>, encoding: Encoding) -> Result<Vec<u8>, StatusCode> {
        let nonce: u32 = rand::rng().random();
        let part = std::env::temp_dir().join(format!("recording-test-{nonce:08x}.part"));

        let result = store(&part, Body::from(body), encoding).await;
        let written = std::fs::read(&part);
        let _ = std::fs::remove_file(&part);

        result.map(|_| written.unwrap())
    }

    #[tokio::test]
    async fn an_uncompressed_upload_is_stored_compressed() {
        let original = recording();

        let file = stored(original.clone(), Encoding::Identity).await.unwrap();

        assert!(file.len() < original.len());
        assert_eq!(zstd::decode_all(&file[..]).unwrap(), original);
    }

    #[tokio::test]
    async fn a_compressed_upload_is_stored_as_sent() {
        let compressed = zstd::encode_all(&recording()[..], 9).unwrap();

        let file = stored(compressed.clone(), Encoding::Zstd).await.unwrap();

        assert_eq!(file, compressed);
    }

    #[tokio::test]
    async fn a_compressed_upload_split_into_bytes_is_accepted() {
        let compressed = zstd::encode_all(&recording()[..], 9).unwrap();
        let bytes: Vec<Result<Vec<u8>, std::io::Error>> =
            compressed.iter().map(|b| Ok(vec![*b])).collect();

        let nonce: u32 = rand::rng().random();
        let part = std::env::temp_dir().join(format!("recording-test-{nonce:08x}.part"));
        let body = Body::from_stream(futures_util::stream::iter(bytes));

        let result = store(&part, body, Encoding::Zstd).await;
        let written = std::fs::read(&part).unwrap();
        let _ = std::fs::remove_file(&part);

        assert!(result.is_ok());
        assert_eq!(written, compressed);
    }

    #[tokio::test]
    async fn a_compressed_upload_that_is_not_a_recording_is_refused() {
        let compressed = zstd::encode_all(&b"NOTAREC\0 and then some"[..], 9).unwrap();

        assert_eq!(
            stored(compressed, Encoding::Zstd).await,
            Err(StatusCode::UNPROCESSABLE_ENTITY)
        );
    }

    #[tokio::test]
    async fn an_upload_that_is_not_zstd_is_refused() {
        assert_eq!(
            stored(recording(), Encoding::Zstd).await,
            Err(StatusCode::UNPROCESSABLE_ENTITY)
        );
    }

    #[tokio::test]
    async fn an_unknown_encoding_is_refused() {
        let mut headers = HeaderMap::new();
        headers.insert("x-iw4x-map", "mp_rust".parse().unwrap());
        headers.insert("x-iw4x-gametype", "war".parse().unwrap());
        headers.insert("x-iw4x-xuid", "1".parse().unwrap());
        headers.insert("x-iw4x-started", "0".parse().unwrap());
        headers.insert(header::CONTENT_ENCODING, "gzip".parse().unwrap());

        assert_eq!(
            upload(headers, Body::from(recording())).await,
            StatusCode::UNSUPPORTED_MEDIA_TYPE
        );
    }
}
