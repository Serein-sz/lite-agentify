use axum::{
    Extension, Json,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use super::{AdminState, SessionIdentity};
use crate::account::Role;
use crate::usage::{
    StatusFilter, SummaryBucket, UsageListParams, UsageRow, UsageSummary, UsageSummaryParams,
};

const DEFAULT_PAGE_SIZE: u64 = 50;
const MAX_PAGE_SIZE: u64 = 200;

#[derive(Deserialize)]
pub(crate) struct ListQuery {
    from: Option<String>,
    to: Option<String>,
    provider: Option<String>,
    model: Option<String>,
    status: Option<String>,
    /// Admin-only filters; ignored (scope-forced) for user-role sessions.
    user: Option<Uuid>,
    api_key: Option<Uuid>,
    page: Option<u64>,
    page_size: Option<u64>,
}

#[derive(Serialize)]
struct ListResponse {
    usage_enabled: bool,
    rows: Vec<UsageRow>,
    total: u64,
    page: u64,
    page_size: u64,
}

pub(crate) async fn list_usage(
    State(state): State<AdminState>,
    Extension(identity): Extension<SessionIdentity>,
    Query(query): Query<ListQuery>,
) -> Response {
    let from = match parse_time(query.from.as_deref(), "from") {
        Ok(value) => value,
        Err(response) => return response,
    };
    let to = match parse_time(query.to.as_deref(), "to") {
        Ok(value) => value,
        Err(response) => return response,
    };
    let status = match parse_status(query.status.as_deref()) {
        Ok(value) => value,
        Err(response) => return response,
    };

    // User-role callers are hard-scoped to their own rows: the session user
    // overrides any requested user filter. Admins may filter freely.
    let user_id = match identity.role {
        Role::Admin => query.user,
        Role::User => Some(identity.user_id),
    };

    let params = UsageListParams {
        from,
        to,
        provider: query.provider,
        model: query.model,
        status,
        user_id,
        api_key_id: query.api_key,
        page: query.page.unwrap_or(1).max(1),
        page_size: query
            .page_size
            .unwrap_or(DEFAULT_PAGE_SIZE)
            .clamp(1, MAX_PAGE_SIZE),
    };
    let (page, page_size) = (params.page, params.page_size);

    let recorder = state.shared().load().usage_recorder.clone();
    let Some(source) = recorder.query() else {
        return Json(ListResponse {
            usage_enabled: false,
            rows: Vec::new(),
            total: 0,
            page,
            page_size,
        })
        .into_response();
    };

    match source.list(params).await {
        Ok(result) => Json(ListResponse {
            usage_enabled: true,
            rows: result.rows,
            total: result.total,
            page,
            page_size,
        })
        .into_response(),
        Err(error) => query_failed(error),
    }
}

#[derive(Deserialize)]
pub(crate) struct SummaryQuery {
    from: Option<String>,
    to: Option<String>,
    bucket: Option<String>,
    /// Admin-only filter; ignored (scope-forced) for user-role sessions.
    user: Option<Uuid>,
}

#[derive(Serialize)]
struct SummaryResponse {
    usage_enabled: bool,
    #[serde(flatten)]
    summary: UsageSummary,
}

pub(crate) async fn usage_summary(
    State(state): State<AdminState>,
    Extension(identity): Extension<SessionIdentity>,
    Query(query): Query<SummaryQuery>,
) -> Response {
    let from = match parse_time(query.from.as_deref(), "from") {
        Ok(value) => value,
        Err(response) => return response,
    };
    let to = match parse_time(query.to.as_deref(), "to") {
        Ok(value) => value,
        Err(response) => return response,
    };
    let bucket = match query.bucket.as_deref() {
        None | Some("day") => SummaryBucket::Day,
        Some("hour") => SummaryBucket::Hour,
        Some(other) => {
            return bad_request(format!("invalid bucket '{other}': expected hour or day"));
        }
    };
    let user_id = match identity.role {
        Role::Admin => query.user,
        Role::User => Some(identity.user_id),
    };

    let recorder = state.shared().load().usage_recorder.clone();
    let Some(source) = recorder.query() else {
        return Json(SummaryResponse {
            usage_enabled: false,
            summary: UsageSummary::default(),
        })
        .into_response();
    };

    match source
        .summary(UsageSummaryParams {
            from,
            to,
            bucket,
            user_id,
        })
        .await
    {
        Ok(summary) => Json(SummaryResponse {
            usage_enabled: true,
            summary,
        })
        .into_response(),
        Err(error) => query_failed(error),
    }
}

fn parse_time(value: Option<&str>, field: &str) -> Result<Option<DateTime<Utc>>, Response> {
    let Some(value) = value else {
        return Ok(None);
    };
    DateTime::parse_from_rfc3339(value)
        .map(|parsed| Some(parsed.with_timezone(&Utc)))
        .map_err(|error| bad_request(format!("invalid {field} timestamp '{value}': {error}")))
}

fn parse_status(value: Option<&str>) -> Result<Option<StatusFilter>, Response> {
    match value {
        None => Ok(None),
        Some("4xx") => Ok(Some(StatusFilter::ClientError)),
        Some("5xx") => Ok(Some(StatusFilter::ServerError)),
        Some(code) => code
            .parse::<u16>()
            .map(|code| Some(StatusFilter::Exact(code)))
            .map_err(|_| {
                bad_request(format!(
                    "invalid status filter '{code}': expected a status code, 4xx, or 5xx"
                ))
            }),
    }
}

fn bad_request(error: String) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))).into_response()
}

fn query_failed(error: anyhow::Error) -> Response {
    tracing::warn!(error = format!("{error:#}"), "admin usage query failed");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": format!("usage query failed: {error}") })),
    )
        .into_response()
}
