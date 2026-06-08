use crate::profile::{ArchiveScope, Profile};
use crate::storage::{ArchiveBackend, ServedProfile};
use crate::{json_wire, range::frame_range, DEFAULT_MAX_RANGE_COUNT, WIRE_VERSION};

const DEFAULT_MAX_CUTTHROUGH_DELTA_BYTES: usize = 100 * 1024;
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
// use tower_http::trace::TraceLayer;

pub struct ServerConfig {
    pub bind: String,
    pub max_range_count: u32,
    pub max_cutthrough_delta_bytes: usize,
}

impl ServerConfig {
    pub fn new(bind: impl Into<String>) -> Self {
        Self {
            bind: bind.into(),
            max_range_count: DEFAULT_MAX_RANGE_COUNT,
            max_cutthrough_delta_bytes: DEFAULT_MAX_CUTTHROUGH_DELTA_BYTES,
        }
    }
}

#[derive(Clone)]
struct AppState {
    archive: Arc<dyn ArchiveBackend>,
    max_range_count: u32,
    max_cutthrough_delta_bytes: usize,
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    error: anyhow::Error,
}

impl ApiError {
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            error: anyhow::anyhow!(message.into()),
        }
    }

    pub fn internal(error: anyhow::Error) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            error,
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
        let body = Json(serde_json::json!({
            "error": self.error.to_string()
        }));

        (self.status, body).into_response()
    }
}

#[derive(Debug, Deserialize)]
struct ClientProfileQuery {
    /// Archive scope requested by the client. Defaults to p2tr-sp when omitted.
    /// If the server did not index the requested scope, the request fails.
    scope: Option<ArchiveScope>,
}

#[derive(Debug, Deserialize)]
struct SyncRangeQuery {
    /// First height the client needs.
    start: u64,
    /// Maximum number of consecutive blocks requested. The server may return
    /// fewer blocks if the profile served tip is reached.
    count: u32,
    /// Archive scope requested by the client. Defaults to p2tr-sp when omitted.
    /// If the server did not index the requested scope, the request fails.
    scope: Option<ArchiveScope>,
}

#[derive(Debug, Deserialize)]
struct CutthroughDeltaQuery {
    /// Highest block the client has already fully applied. The server advances
    /// from known_height + 1 through its current cut-through boundary.
    known_height: u64,
    /// Archive scope requested by the client. Defaults to p2tr-sp when omitted.
    scope: Option<ArchiveScope>,
}

#[derive(Debug, Deserialize)]
struct LatestCheckpointQuery {
    height_lte: u64,
    /// Archive scope requested by the client. Defaults to p2tr-sp when omitted.
    /// If the server did not index the requested scope, the request fails.
    scope: Option<ArchiveScope>,
}

pub async fn serve(config: ServerConfig, archive: Arc<dyn ArchiveBackend>) -> anyhow::Result<()> {
    let state = AppState {
        archive,
        max_range_count: config.max_range_count,
        max_cutthrough_delta_bytes: config.max_cutthrough_delta_bytes,
    };
    let app = Router::new()
        .route("/health", get(health))
        .route("/manifest", get(manifest))
        .route("/tip", get(tip))
        .route("/blocks/light", get(block_range))
        .route("/blocks/light/cutthrough/delta", get(cutthrough_delta_range))
        .route("/blocks/light/cutthrough/snapshot/latest", get(cutthrough_snapshot_latest))
        .route("/blocks/light/cutthrough/snapshot/:file", get(cutthrough_snapshot_by_height))
        .route("/blocks/:height/light", get(single_block))
        .route("/checkpoints/latest", get(latest_checkpoint))
        .route("/checkpoints/:height", get(checkpoint))
        .route("/debug/blocks/:height/stats", get(block_stats))
        // .layer(TraceLayer::new_for_http())
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

async fn tip(
    State(state): State<AppState>,
    Query(q): Query<ClientProfileQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let profile = select_profile(&state.archive, q.scope, StreamProfile::Full, None).await?;
    let tip = state
        .archive
        .tip(&profile)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no served tip for profile {}", profile.name))?;
    Ok(Json(json!({
        "height": tip.height,
        "block_hash": tip.block_hash,
        "profile": profile.name.clone(),
        "scope": profile.profile.scope.as_str(),
        "cutthrough": profile.profile.cutthrough_blocks != 0,
        "cutthrough_blocks": profile.profile.cutthrough_blocks,
    })))
}

async fn single_block(
    State(state): State<AppState>,
    Path(height): Path<u64>,
    Query(q): Query<ClientProfileQuery>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let format = response_format(&headers)?;
    let profile = select_profile(&state.archive, q.scope, StreamProfile::Full, Some(height)).await?;
    ensure_height_served(height, &profile)?;
    let (payload, block_hash) = state.archive.read_block(height, &profile).await?;
    let mut response = match format {
        ResponseFormat::Capnp => binary_response(payload, cache_control(height, &profile)),
        ResponseFormat::Json => json_response(
            serde_json::to_vec_pretty(&json_wire::light_block_to_json(&payload, Some(&profile))?)?,
            cache_control(height, &profile),
        ),
    };
    add_block_headers(&mut response, height, &block_hash, &profile);
    Ok(response)
}

async fn block_range(
    State(state): State<AppState>,
    Query(q): Query<SyncRangeQuery>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    if q.count == 0 {
        return Err(ApiError::bad_request("count must be greater than zero"));
    }
    if q.count > state.max_range_count {
        return Err(ApiError::bad_request(format!(
            "count {} exceeds max_range_count {}",
            q.count, state.max_range_count
        )));
    }
    let format = response_format(&headers)?;
    let profile = select_profile(&state.archive, q.scope, StreamProfile::Full, Some(q.start)).await?;
    let tip = profile
        .served_tip
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("profile {} has no served tip", profile.name))?;
    if q.start > tip.height {
        return Err(ApiError::bad_request(format!(
            "start height {} is above served tip {} for profile {}",
            q.start, tip.height, profile.name
        )));
    }
    let available = tip.height - q.start + 1;
    let count = q.count.min(u32::try_from(available).unwrap_or(u32::MAX));
    let end = q.start + u64::from(count) - 1;
    let messages = state.archive.read_blocks(q.start, count, &profile).await?;
    let mut response = match format {
        ResponseFormat::Capnp => {
            binary_response(frame_range(&messages)?, cache_control(end, &profile))
        }
        ResponseFormat::Json => {
            let blocks = messages
                .iter()
                .map(|payload| json_wire::light_block_to_json(payload, Some(&profile)))
                .collect::<anyhow::Result<Vec<_>>>()?;
            let body = serde_json::to_vec_pretty(&json!({
                "version": WIRE_VERSION,
                "format": "light-block-range",
                "profile": profile.name.clone(),
                "scope": profile.profile.scope.as_str(),
                "cutthrough": profile.profile.cutthrough_blocks != 0,
                "cutthrough_blocks": profile.profile.cutthrough_blocks,
                "start": q.start,
                "end": end,
                "count": count,
                "blocks": blocks,
            }))?;
            json_response(body, cache_control(end, &profile))
        }
    };
    add_range_headers(&mut response, q.start, end, count, &profile);
    Ok(response)
}

async fn cutthrough_delta_range(
    State(state): State<AppState>,
    Query(q): Query<CutthroughDeltaQuery>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let format = response_format(&headers)?;
    let profile = select_delta_profile(&state.archive, q.scope).await?;
    let tip = profile
        .served_tip
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("cut-through profile for scope has no served tip"))?;
    let cutthrough_tip_height = tip.height;

    if q.known_height >= cutthrough_tip_height {
        return Err(ApiError::bad_request(format!(
            "known_height {} is at or above cut-through tip {} for scope {}; use /blocks/light for the full stream",
            q.known_height,
            cutthrough_tip_height,
            profile.profile.scope.as_str()
        )));
    }

    let max_end_height = q
        .known_height
        .saturating_add(u64::from(state.max_range_count))
        .min(cutthrough_tip_height);

    let delta = state
        .archive
        .read_cutthrough_delta_blocks(
            q.known_height,
            max_end_height,
            state.max_cutthrough_delta_bytes,
            &profile,
        )
        .await?;
    let count = u32::try_from(delta.messages.len()).map_err(|err| ApiError::internal(err.into()))?;
    let mut response = match format {
        ResponseFormat::Capnp => binary_response(frame_range(&delta.messages)?, "no-cache"),
        ResponseFormat::Json => {
            let blocks = delta
                .messages
                .iter()
                .map(|payload| json_wire::light_block_to_json(payload, Some(&profile)))
                .collect::<anyhow::Result<Vec<_>>>()?;
            let body = serde_json::to_vec_pretty(&json!({
                "version": WIRE_VERSION,
                "format": "light-block-cutthrough-delta-range",
                "profile": profile.name.clone(),
                "scope": profile.profile.scope.as_str(),
                "stream": "cutthrough-delta",
                "known_height": q.known_height,
                "start": q.known_height + 1,
                "end": delta.end_height,
                "cutthrough_tip": cutthrough_tip_height,
                "target_response_bytes": state.max_cutthrough_delta_bytes,
                "count": count,
                "blocks": blocks,
            }))?;
            json_response(body, "no-cache")
        }
    };
    add_cutthrough_delta_headers(
        &mut response,
        q.known_height,
        delta.end_height,
        cutthrough_tip_height,
        state.max_cutthrough_delta_bytes,
        count,
        &profile,
    );
    Ok(response)
}

async fn cutthrough_snapshot_latest(
    State(state): State<AppState>,
    Query(q): Query<ClientProfileQuery>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let format = response_format(&headers)?;
    if format == ResponseFormat::Json {
        return Err(ApiError::bad_request(
            "cut-through snapshot endpoints serve binary BDSS only; use Accept: application/octet-stream",
        ));
    }
    let profile = select_delta_profile(&state.archive, q.scope).await?;
    let tip = profile
        .served_tip
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("cut-through profile for scope has no served tip"))?;
    let snapshot = state
        .archive
        .read_cutthrough_snapshot(tip.height, &profile)
        .await?;
    let mut response = binary_response(snapshot.payload.clone(), "no-cache");
    add_cutthrough_snapshot_headers(&mut response, &snapshot, &profile);
    Ok(response)
}

async fn cutthrough_snapshot_by_height(
    State(state): State<AppState>,
    Path(file): Path<String>,
    Query(q): Query<ClientProfileQuery>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let format = response_format(&headers)?;
    if format == ResponseFormat::Json {
        return Err(ApiError::bad_request(
            "cut-through snapshot endpoints serve binary BDSS only; use Accept: application/octet-stream",
        ));
    }
    let height_text = file.strip_suffix(".bdss").unwrap_or(&file);
    let height = height_text
        .parse::<u64>()
        .map_err(|_| ApiError::bad_request("snapshot file must be {height}.bdss"))?;
    let profile = select_profile(&state.archive, q.scope, StreamProfile::CutThrough, Some(height)).await?;
    ensure_height_served(height, &profile)?;
    ensure_cutthrough_height_allowed(&state.archive, height, q.scope).await?;
    let snapshot = state
        .archive
        .read_cutthrough_snapshot(height, &profile)
        .await?;
    let mut response = binary_response(snapshot.payload.clone(), cache_control(height, &profile));
    add_cutthrough_snapshot_headers(&mut response, &snapshot, &profile);
    Ok(response)
}

async fn checkpoint(
    State(state): State<AppState>,
    Path(height): Path<u64>,
    Query(q): Query<ClientProfileQuery>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let format = response_format(&headers)?;
    let profile = select_profile(&state.archive, q.scope, StreamProfile::Full, Some(height)).await?;
    ensure_height_served(height, &profile)?;
    let body = state.archive.read_checkpoint(height, &profile).await?;
    let mut response = match format {
        ResponseFormat::Capnp => binary_response(body, cache_control(height, &profile)),
        ResponseFormat::Json => json_response(
            serde_json::to_vec_pretty(&json_wire::checkpoint_to_json(&body, Some(&profile))?)?,
            cache_control(height, &profile),
        ),
    };
    add_checkpoint_headers(&mut response, height, &profile);
    Ok(response)
}

async fn latest_checkpoint(
    State(state): State<AppState>,
    Query(q): Query<LatestCheckpointQuery>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let format = response_format(&headers)?;
    let profile = select_profile(&state.archive, q.scope, StreamProfile::Full, Some(q.height_lte)).await?;
    let height_lte = match profile.served_tip.as_ref() {
        Some(tip) => q.height_lte.min(tip.height),
        None => q.height_lte,
    };
    let height = state
        .archive
        .latest_checkpoint_height(height_lte, &profile)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no checkpoint <= {height_lte}"))?;
    let body = state.archive.read_checkpoint(height, &profile).await?;
    let mut response = match format {
        ResponseFormat::Capnp => binary_response(body, cache_control(height, &profile)),
        ResponseFormat::Json => json_response(
            serde_json::to_vec_pretty(&json_wire::checkpoint_to_json(&body, Some(&profile))?)?,
            cache_control(height, &profile),
        ),
    };
    add_checkpoint_headers(&mut response, height, &profile);
    Ok(response)
}

async fn block_stats(
    State(state): State<AppState>,
    Path(height): Path<u64>,
) -> ApiResult<Json<serde_json::Value>> {
    Ok(Json(state.archive.block_stats(height).await?))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StreamProfile {
    Full,
    CutThrough,
}

async fn select_profile(
    archive: &Arc<dyn ArchiveBackend>,
    requested_scope: Option<ArchiveScope>,
    stream: StreamProfile,
    start_or_height: Option<u64>,
) -> anyhow::Result<ServedProfile> {
    let scope = requested_scope.unwrap_or(ArchiveScope::P2trSp);
    let manifest = archive.manifest().await?;
    let indexed_scopes: std::collections::BTreeSet<_> =
        manifest.profiles.iter().map(|p| p.scope).collect();
    anyhow::ensure!(
        indexed_scopes.contains(&scope),
        "scope {} is not indexed by this server; indexed scopes: {}",
        scope.as_str(),
        indexed_scopes
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );

    if stream == StreamProfile::Full {
        return archive
            .resolve_profile(
                None,
                Some(Profile {
                    scope,
                    cutthrough_blocks: 0,
                }),
            )
            .await
            .map_err(|err| {
                anyhow::anyhow!(
                    "full profile for scope {} is not available on this server: {err}",
                    scope.as_str()
                )
            });
    }

    let min_height = start_or_height.unwrap_or(0);

    let mut candidates: Vec<_> = manifest
        .profiles
        .into_iter()
        .filter(|p| p.scope == scope && p.cutthrough_blocks > 0)
        .filter(|p| p.tip.as_ref().is_some_and(|tip| tip.height >= min_height))
        .collect();

    candidates.sort_by_key(|p| p.cutthrough_blocks);
    let selected = candidates.pop()
        .ok_or_else(|| anyhow::anyhow!(
            "no cut-through profile for scope {} can serve start/height {}; use /blocks/light for the full stream or a lower start height",
            scope.as_str(),
            min_height
        ))?;

    archive
        .resolve_profile(
            None,
            Some(Profile {
                scope,
                cutthrough_blocks: selected.cutthrough_blocks,
            }),
        )
        .await
}

async fn select_delta_profile(
    archive: &Arc<dyn ArchiveBackend>,
    requested_scope: Option<ArchiveScope>,
) -> anyhow::Result<ServedProfile> {
    select_profile(archive, requested_scope, StreamProfile::CutThrough, None).await
}

async fn ensure_cutthrough_height_allowed(
    archive: &Arc<dyn ArchiveBackend>,
    height: u64,
    requested_scope: Option<ArchiveScope>,
) -> ApiResult<()> {
    let scope = requested_scope.unwrap_or(ArchiveScope::P2trSp);
    let manifest = archive.manifest().await?;
    let Some(full_profile) = manifest
        .profiles
        .iter()
        .find(|p| p.scope == scope && p.cutthrough_blocks == 0)
    else {
        return Ok(());
    };
    let Some(tip) = &full_profile.tip else {
        return Ok(());
    };
    let max_cutthrough_height = tip.height.saturating_sub(manifest.suggested_reorg_cache_depth);
    if height > max_cutthrough_height {
        return Err(ApiError::bad_request(format!(
            "cut-through requests must end at or below height {max_cutthrough_height} for scope {}; full tip is {}, recent_full_depth is {}",
            scope.as_str(),
            tip.height,
            manifest.suggested_reorg_cache_depth
        )));
    }
    Ok(())
}

fn ensure_height_served(height: u64, profile: &ServedProfile) -> anyhow::Result<()> {
    let Some(tip) = &profile.served_tip else {
        anyhow::bail!("profile {} has no served tip", profile.name);
    };
    anyhow::ensure!(
        height <= tip.height,
        "height {height} is above served tip {} for profile {}",
        tip.height,
        profile.name
    );
    Ok(())
}

fn cache_control(height: u64, profile: &ServedProfile) -> &'static str {
    match profile.served_tip.as_ref() {
        Some(tip) if height <= tip.height => "public, max-age=31536000, immutable",
        _ => "no-cache",
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
    if accept.contains("application/json") || accept.contains("text/json") {
        return Ok(ResponseFormat::Json);
    }
    if accept.contains("application/octet-stream") || accept.contains("*/*") {
        return Ok(ResponseFormat::Capnp);
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
    profile: &ServedProfile,
) {
    response.headers_mut().insert(
        HeaderName::from_static("x-bitcoindata-light-version"),
        HeaderValue::from_str(&WIRE_VERSION.to_string()).unwrap(),
    );
    response.headers_mut().insert(
        HeaderName::from_static("x-bitcoindata-profile"),
        HeaderValue::from_str(&profile.name).unwrap(),
    );
    response.headers_mut().insert(
        HeaderName::from_static("x-bitcoindata-scope"),
        HeaderValue::from_str(profile.profile.scope.as_str()).unwrap(),
    );
    response.headers_mut().insert(
        HeaderName::from_static("x-bitcoindata-cutthrough"),
        HeaderValue::from_static(if profile.profile.cutthrough_blocks == 0 {
            "false"
        } else {
            "true"
        }),
    );
    response.headers_mut().insert(
        HeaderName::from_static("x-bitcoindata-cutthrough-blocks"),
        HeaderValue::from_str(&profile.profile.cutthrough_blocks.to_string()).unwrap(),
    );
    response.headers_mut().insert(
        HeaderName::from_static("x-bitcoindata-stream"),
        HeaderValue::from_static(if profile.profile.cutthrough_blocks == 0 {
            "full"
        } else {
            "cutthrough"
        }),
    );
    response.headers_mut().insert(
        HeaderName::from_static("x-bitcoin-block-height"),
        HeaderValue::from_str(&height.to_string()).unwrap(),
    );
    response.headers_mut().insert(
        HeaderName::from_static("x-bitcoin-block-hash"),
        HeaderValue::from_str(&hex::encode(block_hash)).unwrap(),
    );
}

fn add_range_headers(
    response: &mut Response,
    start: u64,
    end: u64,
    count: u32,
    profile: &ServedProfile,
) {
    response.headers_mut().insert(
        HeaderName::from_static("x-bitcoindata-profile"),
        HeaderValue::from_str(&profile.name).unwrap(),
    );
    response.headers_mut().insert(
        HeaderName::from_static("x-bitcoindata-scope"),
        HeaderValue::from_str(profile.profile.scope.as_str()).unwrap(),
    );
    response.headers_mut().insert(
        HeaderName::from_static("x-bitcoindata-cutthrough"),
        HeaderValue::from_static(if profile.profile.cutthrough_blocks == 0 {
            "false"
        } else {
            "true"
        }),
    );
    response.headers_mut().insert(
        HeaderName::from_static("x-bitcoindata-cutthrough-blocks"),
        HeaderValue::from_str(&profile.profile.cutthrough_blocks.to_string()).unwrap(),
    );
    response.headers_mut().insert(
        HeaderName::from_static("x-bitcoindata-stream"),
        HeaderValue::from_static(if profile.profile.cutthrough_blocks == 0 {
            "full"
        } else {
            "cutthrough"
        }),
    );
    response.headers_mut().insert(
        HeaderName::from_static("x-bitcoindata-range-start"),
        HeaderValue::from_str(&start.to_string()).unwrap(),
    );
    response.headers_mut().insert(
        HeaderName::from_static("x-bitcoindata-range-end"),
        HeaderValue::from_str(&end.to_string()).unwrap(),
    );
    response.headers_mut().insert(
        HeaderName::from_static("x-bitcoindata-range-count"),
        HeaderValue::from_str(&count.to_string()).unwrap(),
    );
}

fn add_cutthrough_delta_headers(
    response: &mut Response,
    known_height: u64,
    end: u64,
    cutthrough_tip_height: u64,
    target_response_bytes: usize,
    count: u32,
    profile: &ServedProfile,
) {
    response.headers_mut().insert(
        HeaderName::from_static("x-bitcoindata-profile"),
        HeaderValue::from_str(&profile.name).unwrap(),
    );
    response.headers_mut().insert(
        HeaderName::from_static("x-bitcoindata-scope"),
        HeaderValue::from_str(profile.profile.scope.as_str()).unwrap(),
    );
    response.headers_mut().insert(
        HeaderName::from_static("x-bitcoindata-stream"),
        HeaderValue::from_static("cutthrough-delta"),
    );
    response.headers_mut().insert(
        HeaderName::from_static("x-bitcoindata-known-height"),
        HeaderValue::from_str(&known_height.to_string()).unwrap(),
    );
    response.headers_mut().insert(
        HeaderName::from_static("x-bitcoindata-range-start"),
        HeaderValue::from_str(&(known_height + 1).to_string()).unwrap(),
    );
    response.headers_mut().insert(
        HeaderName::from_static("x-bitcoindata-range-end"),
        HeaderValue::from_str(&end.to_string()).unwrap(),
    );
    response.headers_mut().insert(
        HeaderName::from_static("x-bitcoindata-delta-end-height"),
        HeaderValue::from_str(&end.to_string()).unwrap(),
    );
    response.headers_mut().insert(
        HeaderName::from_static("x-bitcoindata-cutthrough-tip-height"),
        HeaderValue::from_str(&cutthrough_tip_height.to_string()).unwrap(),
    );
    response.headers_mut().insert(
        HeaderName::from_static("x-bitcoindata-target-response-bytes"),
        HeaderValue::from_str(&target_response_bytes.to_string()).unwrap(),
    );
    response.headers_mut().insert(
        HeaderName::from_static("x-bitcoindata-range-count"),
        HeaderValue::from_str(&count.to_string()).unwrap(),
    );
}

fn add_cutthrough_snapshot_headers(
    response: &mut Response,
    snapshot: &crate::storage::CutthroughSnapshot,
    profile: &ServedProfile,
) {
    response.headers_mut().insert(
        HeaderName::from_static("x-bitcoindata-profile"),
        HeaderValue::from_str(&profile.name).unwrap(),
    );
    response.headers_mut().insert(
        HeaderName::from_static("x-bitcoindata-scope"),
        HeaderValue::from_str(profile.profile.scope.as_str()).unwrap(),
    );
    response.headers_mut().insert(
        HeaderName::from_static("x-bitcoindata-stream"),
        HeaderValue::from_static("cutthrough-snapshot"),
    );
    response.headers_mut().insert(
        HeaderName::from_static("x-bitcoindata-snapshot-format"),
        HeaderValue::from_static("BDSS"),
    );
    response.headers_mut().insert(
        HeaderName::from_static("x-bitcoindata-snapshot-height"),
        HeaderValue::from_str(&snapshot.height.to_string()).unwrap(),
    );
    response.headers_mut().insert(
        HeaderName::from_static("x-bitcoin-block-hash"),
        HeaderValue::from_str(&snapshot.block_hash).unwrap(),
    );
    response.headers_mut().insert(
        HeaderName::from_static("x-bitcoindata-snapshot-block-count"),
        HeaderValue::from_str(&snapshot.block_count.to_string()).unwrap(),
    );
    response.headers_mut().insert(
        HeaderName::from_static("x-bitcoindata-cutthrough-blocks"),
        HeaderValue::from_str(&profile.profile.cutthrough_blocks.to_string()).unwrap(),
    );
}

fn add_checkpoint_headers(response: &mut Response, height: u64, profile: &ServedProfile) {
    response.headers_mut().insert(
        HeaderName::from_static("x-bitcoin-checkpoint-height"),
        HeaderValue::from_str(&height.to_string()).unwrap(),
    );
    response.headers_mut().insert(
        HeaderName::from_static("x-bitcoindata-profile"),
        HeaderValue::from_str(&profile.name).unwrap(),
    );
    response.headers_mut().insert(
        HeaderName::from_static("x-bitcoindata-scope"),
        HeaderValue::from_str(profile.profile.scope.as_str()).unwrap(),
    );
    response.headers_mut().insert(
        HeaderName::from_static("x-bitcoindata-cutthrough-blocks"),
        HeaderValue::from_str(&profile.profile.cutthrough_blocks.to_string()).unwrap(),
    );
    response.headers_mut().insert(
        HeaderName::from_static("x-bitcoindata-stream"),
        HeaderValue::from_static(if profile.profile.cutthrough_blocks == 0 {
            "full"
        } else {
            "cutthrough"
        }),
    );
}
