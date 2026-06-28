use crate::index::StoredBlockResponseFilter;
use crate::storage::{ArchiveBackend, ChainTip, Manifest};
use crate::{
    json_wire, range::frame_range, DEFAULT_MAX_RANGE_COUNT, DEFAULT_MAX_RESPONSE_BYTES,
    WIRE_VERSION,
};

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use std::net::SocketAddr;
use std::sync::Arc;

pub struct ServerConfig {
    pub bind: String,
    pub max_range_count: u32,
    pub max_response_bytes: usize,
}

impl ServerConfig {
    pub fn new(bind: impl Into<String>) -> Self {
        Self {
            bind: bind.into(),
            max_range_count: DEFAULT_MAX_RANGE_COUNT,
            max_response_bytes: DEFAULT_MAX_RESPONSE_BYTES,
        }
    }
}

#[derive(Clone)]
struct AppState {
    archive: Arc<dyn ArchiveBackend>,
    max_range_count: u32,
    max_response_bytes: usize,
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    error: String,
}

impl ApiError {
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            error: message.into(),
        }
    }

    pub fn internal(error: anyhow::Error) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            error: error.to_string(),
        }
    }
}

type ApiResult<T> = Result<T, ApiError>;

impl From<anyhow::Error> for ApiError {
    fn from(error: anyhow::Error) -> Self {
        Self::internal(error)
    }
}

impl From<serde_json::Error> for ApiError {
    fn from(error: serde_json::Error) -> Self {
        Self::internal(anyhow::Error::from(error))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = Json(serde_json::json!({ "error": self.error }));
        (self.status, body).into_response()
    }
}

#[derive(Debug, Deserialize)]
struct SyncRangeQuery {
    /// First height the client needs.
    start: u64,
    /// Maximum number of consecutive blocks requested. If omitted, the server
    /// uses max_range_count.
    count: Option<u32>,
    /// Enable cut-through using `start` as the cut-through boundary.
    #[serde(default)]
    cutthrough: bool,
    /// Explicit cut-through boundary. Takes precedence over `cutthrough=true`.
    cutthrough_start: Option<u64>,
    /// Omit outputs marked reused in the storage block.
    #[serde(default)]
    filter_reuse: bool,
    /// Requested label budget. `labels <= 2` serves the two-label fingerprint stream;
    /// any larger value or omission serves the hundred-label stream.
    labels: Option<u16>,
    /// Optional per-request response byte cap. Must not exceed the server cap.
    max_bytes: Option<usize>,
}

#[derive(Debug, Default, Deserialize)]
struct SingleBlockQuery {
    /// Requested label budget. `labels <= 2` serves the two-label fingerprint stream;
    /// any larger value or omission serves the hundred-label stream.
    labels: Option<u16>,
}

pub async fn serve(config: ServerConfig, archive: Arc<dyn ArchiveBackend>) -> anyhow::Result<()> {
    let state = AppState {
        archive,
        max_range_count: config.max_range_count,
        max_response_bytes: config.max_response_bytes,
    };

    let app = Router::new()
        .route("/health", get(health))
        .route("/manifest", get(manifest))
        .route("/tip", get(tip))
        .route("/blocks/light", get(block_range))
        .route("/blocks/{height}/light", get(single_block))
        .with_state(state);

    let addr: SocketAddr = config.bind.parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({ "ok": true, "version": WIRE_VERSION }))
}

async fn manifest(State(state): State<AppState>) -> ApiResult<Json<serde_json::Value>> {
    Ok(Json(serde_json::to_value(state.archive.manifest().await?)?))
}

async fn tip(State(state): State<AppState>) -> ApiResult<Json<serde_json::Value>> {
    let tip = state
        .archive
        .tip()
        .await?
        .ok_or_else(|| anyhow::anyhow!("archive has no served tip"))?;
    Ok(Json(json!({
        "height": tip.height,
        "block_hash": tip.block_hash,
    })))
}

async fn single_block(
    State(state): State<AppState>,
    Path(height): Path<u64>,
    Query(q): Query<SingleBlockQuery>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let format = response_format(&headers)?;
    let tip = served_tip(&state.archive).await?;
    ensure_height_served(height, &tip)?;
    let manifest = state.archive.manifest().await?;

    let served = state
        .archive
        .read_block_filtered(
            height,
            StoredBlockResponseFilter {
                labels: q.labels,
                ..StoredBlockResponseFilter::default()
            },
        )
        .await?;
    let cache_control = cache_control(&manifest, &tip, height);
    let mut response = match format {
        ResponseFormat::Capnp => binary_response(served.payload, cache_control),
        ResponseFormat::Json => json_response(
            serde_json::to_vec_pretty(&json_wire::light_block_to_json(&served.payload)?)?,
            cache_control,
        ),
    };
    add_block_headers(&mut response, height, served.block_hash.as_bytes())?;
    Ok(response)
}

async fn block_range(
    State(state): State<AppState>,
    Query(q): Query<SyncRangeQuery>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let requested_count = q.count.unwrap_or(state.max_range_count);
    if requested_count == 0 {
        return Err(ApiError::bad_request("count must be greater than zero"));
    }
    if requested_count > state.max_range_count {
        return Err(ApiError::bad_request(format!(
            "count {} exceeds max_range_count {}",
            requested_count, state.max_range_count
        )));
    }

    let max_response_bytes = q.max_bytes.unwrap_or(state.max_response_bytes);
    if max_response_bytes == 0 {
        return Err(ApiError::bad_request("max_bytes must be greater than zero"));
    }
    if max_response_bytes > state.max_response_bytes {
        return Err(ApiError::bad_request(format!(
            "max_bytes {} exceeds server max_response_bytes {}",
            max_response_bytes, state.max_response_bytes
        )));
    }

    let format = response_format(&headers)?;
    let tip = served_tip(&state.archive).await?;
    let manifest = state.archive.manifest().await?;
    if q.start > tip.height {
        return Err(ApiError::bad_request(format!(
            "start height {} is above served tip {}",
            q.start, tip.height
        )));
    }

    let available = tip.height - q.start + 1;
    let requested_count = requested_count.min(u32::try_from(available).unwrap_or(u32::MAX));
    let requested_end = q.start + u64::from(requested_count) - 1;
    let cutthrough_start = q
        .cutthrough_start
        .or_else(|| q.cutthrough.then_some(q.start));
    if let Some(cutthrough_start) = cutthrough_start {
        if cutthrough_start > tip.height {
            return Err(ApiError::bad_request(format!(
                "cutthrough_start {} is above served tip {}",
                cutthrough_start, tip.height
            )));
        }
    }

    let filter = StoredBlockResponseFilter {
        cutthrough_start,
        cutthrough_tip: cutthrough_start.map(|_| tip.height),
        filter_reuse: q.filter_reuse,
        labels: q.labels,
    };

    let mut messages = Vec::<Vec<u8>>::new();
    let mut framed_bytes = range_frame_header_len();
    for offset in 0..requested_count {
        let height = q.start + u64::from(offset);
        let served = state.archive.read_block_filtered(height, filter).await?;
        let projected = framed_bytes
            .checked_add(range_frame_item_len(served.payload.len()))
            .ok_or_else(|| anyhow::anyhow!("range response size overflow"))?;

        if !messages.is_empty() && projected > max_response_bytes {
            break;
        }

        framed_bytes = projected;
        messages.push(served.payload);

        // Always include at least one block so a client can make progress even
        // if a single block is larger than the configured response cap.
        if framed_bytes > max_response_bytes {
            break;
        }
    }

    if messages.is_empty() {
        return Err(anyhow::anyhow!(
            "range response builder made no progress from start height {}",
            q.start
        )
        .into());
    }

    let count = u32::try_from(messages.len()).map_err(anyhow::Error::from)?;
    let end = q.start + u64::from(count) - 1;
    let complete = end >= requested_end;
    let next_start = (!complete).then_some(end + 1);
    let cache_control = cache_control(&manifest, &tip, end);

    let mut response = match format {
        ResponseFormat::Capnp => {
            let body = frame_range(&messages)?;
            binary_response(body, cache_control)
        }
        ResponseFormat::Json => {
            let blocks = messages
                .iter()
                .map(|payload| json_wire::light_block_to_json(payload))
                .collect::<anyhow::Result<Vec<_>>>()?;
            let body = serde_json::to_vec_pretty(&json!({
                "version": WIRE_VERSION,
                "format": "light-block-range",
                "start": q.start,
                "end": end,
                "requested_end": requested_end,
                "count": count,
                "complete": complete,
                "next_start": next_start,
                "blocks": blocks,
            }))?;
            json_response(body, cache_control)
        }
    };
    add_range_headers(
        &mut response,
        RangeHeaderInfo {
            start: q.start,
            end,
            requested_end,
            count,
            complete,
            next_start,
        },
    )?;
    Ok(response)
}

fn range_frame_header_len() -> usize {
    4 + 2 + 4
}

fn range_frame_item_len(payload_len: usize) -> usize {
    4 + payload_len
}

async fn served_tip(archive: &Arc<dyn ArchiveBackend>) -> anyhow::Result<ChainTip> {
    archive
        .tip()
        .await?
        .ok_or_else(|| anyhow::anyhow!("archive has no served tip"))
}

fn ensure_height_served(height: u64, tip: &ChainTip) -> anyhow::Result<()> {
    anyhow::ensure!(
        height <= tip.height,
        "height {height} is above served tip {}",
        tip.height
    );
    Ok(())
}

fn cache_control(manifest: &Manifest, tip: &ChainTip, height: u64) -> &'static str {
    if tip.height.saturating_sub(height) >= manifest.finality_depth {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResponseFormat {
    Capnp,
    Json,
}

fn response_format(headers: &HeaderMap) -> anyhow::Result<ResponseFormat> {
    let Some(accept) = headers.get(header::ACCEPT).and_then(|v| v.to_str().ok()) else {
        return Ok(ResponseFormat::Capnp);
    };

    for media_type in accept
        .split(',')
        .map(|part| part.split(';').next().unwrap_or("").trim())
    {
        match media_type {
            "application/json" | "text/json" => return Ok(ResponseFormat::Json),
            "application/octet-stream" | "*/*" => return Ok(ResponseFormat::Capnp),
            _ => {}
        }
    }

    anyhow::bail!("unsupported Accept header; use application/octet-stream or application/json")
}

fn binary_response(body: Vec<u8>, cache_control: &'static str) -> Response {
    response_with_content_type(body, cache_control, "application/octet-stream")
}

fn json_response(body: Vec<u8>, cache_control: &'static str) -> Response {
    response_with_content_type(body, cache_control, "application/json; charset=utf-8")
}

fn response_with_content_type(
    body: Vec<u8>,
    cache_control: &'static str,
    content_type: &'static str,
) -> Response {
    let mut response = Response::new(Body::from(body));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(cache_control),
    );
    response
        .headers_mut()
        .insert(header::VARY, HeaderValue::from_static("Accept"));
    response
}

fn add_block_headers(
    response: &mut Response,
    height: u64,
    block_hash: &[u8],
) -> anyhow::Result<()> {
    response.headers_mut().insert(
        HeaderName::from_static("x-bitcoindata-light-version"),
        HeaderValue::from_str(&WIRE_VERSION.to_string())?,
    );
    response.headers_mut().insert(
        HeaderName::from_static("x-bitcoin-block-height"),
        HeaderValue::from_str(&height.to_string())?,
    );
    if !block_hash.is_empty() {
        response.headers_mut().insert(
            HeaderName::from_static("x-bitcoin-block-hash"),
            HeaderValue::from_str(&hex::encode(block_hash))?,
        );
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct RangeHeaderInfo {
    start: u64,
    end: u64,
    requested_end: u64,
    count: u32,
    complete: bool,
    next_start: Option<u64>,
}

fn add_range_headers(response: &mut Response, info: RangeHeaderInfo) -> anyhow::Result<()> {
    response.headers_mut().insert(
        HeaderName::from_static("x-bitcoindata-range-start"),
        HeaderValue::from_str(&info.start.to_string())?,
    );
    response.headers_mut().insert(
        HeaderName::from_static("x-bitcoindata-range-end"),
        HeaderValue::from_str(&info.end.to_string())?,
    );
    response.headers_mut().insert(
        HeaderName::from_static("x-bitcoindata-range-count"),
        HeaderValue::from_str(&info.count.to_string())?,
    );
    response.headers_mut().insert(
        HeaderName::from_static("x-bitcoindata-requested-range-end"),
        HeaderValue::from_str(&info.requested_end.to_string())?,
    );
    response.headers_mut().insert(
        HeaderName::from_static("x-bitcoindata-range-complete"),
        HeaderValue::from_static(if info.complete { "true" } else { "false" }),
    );
    if let Some(next_start) = info.next_start {
        response.headers_mut().insert(
            HeaderName::from_static("x-bitcoindata-next-start"),
            HeaderValue::from_str(&next_start.to_string())?,
        );
    }
    Ok(())
}
