//! What each request asks for. The guards run first, in the order ADR 0036 gives them, and every
//! call after them is one method of the service the daemon already runs.

use bytes::Bytes;
use http_body_util::{BodyExt, Limited, StreamBody};
use hyper::body::{Frame, Incoming};
use hyper::{Method, Request, Response};
use serde_json::{Value, json};
use sluice_api::{ApiError, proto};
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::ReceiverStream;

use super::guard::{self, MAX_BODY_BYTES, Reject};
use super::{Body, State, assets, error, full, json, qr, respond};

const API: &str = "/api/v1";

const fn reject(status: u16, message: &'static str) -> Reject {
    Reject { status, message }
}

/// Runs one request through the guards and then the call it names.
pub async fn route(state: &State, request: Request<Incoming>) -> Result<Response<Body>, Reject> {
    let headers = request.headers();
    let text = |name: &str| headers.get(name).and_then(|value| value.to_str().ok());
    guard::check_host(text("host"), state.port)?;
    guard::check_origin(text("origin"), state.port)?;
    // A preflight is an OPTIONS, and there is no answer to it: no method but these two is served.
    let method = request.method().clone();
    if method != Method::GET && method != Method::POST {
        return Err(reject(405, "only GET and POST are served"));
    }
    let path = request.uri().path().to_string();
    if method == Method::GET
        && let Some((kind, body)) = assets::find(&path)
    {
        return Ok(respond(200, kind, full(body)));
    }
    let Some(call) = path.strip_prefix(API) else {
        return Err(reject(404, "there is nothing at this address"));
    };
    guard::check_key(text("authorization"), &state.key)?;
    let body = if method == Method::POST {
        guard::check_json(text("content-type"))?;
        guard::check_length(text("content-length"))?;
        read_body(request).await?
    } else {
        Bytes::new()
    };
    Ok(dispatch(state, &method, call, &body).await)
}

/// The body of a POST, never more than the limit, whatever its framing says.
async fn read_body(request: Request<Incoming>) -> Result<Bytes, Reject> {
    Limited::new(request.into_body(), MAX_BODY_BYTES)
        .collect()
        .await
        .map(|collected| collected.to_bytes())
        .map_err(|_| reject(413, "the request body is larger than 64 KiB"))
}

async fn dispatch(state: &State, method: &Method, call: &str, body: &Bytes) -> Response<Body> {
    let service = &state.service;
    match (method.as_str(), call) {
        ("GET", "/version") => ok(&json::version(&service.version())),
        ("GET", "/roots") => ok(&json::roots(&service.list_roots())),
        ("GET", "/jobs") => ok(&json::jobs(
            &service.list_jobs(&proto::ListJobsRequest::default()),
        )),
        ("GET", "/events") => events(state),
        ("POST", "/entries") => entries(state, body).await,
        ("POST", "/send") => send(state, body).await,
        ("POST", "/inspect") => inspect(state, body).await,
        ("POST", "/receive") => receive(state, body).await,
        ("POST", job) if job.starts_with("/jobs/") && job.ends_with("/cancel") => {
            cancel(state, &job["/jobs/".len()..job.len() - "/cancel".len()])
        }
        _ => error(404, "there is nothing at this address"),
    }
}

fn ok(value: &Value) -> Response<Body> {
    respond(200, "application/json", full(value.to_string()))
}

/// An error of the service as the status that goes with it and its own sentence.
fn failed(error_value: &ApiError) -> Response<Body> {
    let status = match error_value {
        ApiError::Invalid(_) => 400,
        ApiError::Refused(_) => 403,
        ApiError::NotFound(_) => 404,
        ApiError::Busy(_) => 429,
        ApiError::Unavailable(_) => 503,
        ApiError::Failed(_) => 500,
    };
    error(status, &error_value.to_string())
}

fn parse(body: &Bytes) -> Result<Value, Reject> {
    serde_json::from_slice(body)
        .map_err(|_| reject(400, "the request body is not the JSON this call takes"))
}

fn wrong_shape() -> Response<Body> {
    error(
        400,
        "the request body does not have the fields this call takes",
    )
}

async fn entries(state: &State, body: &Bytes) -> Response<Body> {
    let value = match parse(body) {
        Ok(value) => value,
        Err(refused) => return error(refused.status, refused.message),
    };
    let Some(path) = json::root_path(&value) else {
        return wrong_shape();
    };
    match state
        .service
        .list_entries(proto::ListEntriesRequest { path: Some(path) })
        .await
    {
        Ok(listed) => ok(&json::entries(&listed)),
        Err(e) => failed(&e),
    }
}

async fn send(state: &State, body: &Bytes) -> Response<Body> {
    let value = match parse(body) {
        Ok(value) => value,
        Err(refused) => return error(refused.status, refused.message),
    };
    let Some(paths) = value.get("paths").and_then(Value::as_array) else {
        return wrong_shape();
    };
    let Some(paths) = paths
        .iter()
        .map(json::root_path)
        .collect::<Option<Vec<_>>>()
    else {
        return wrong_shape();
    };
    let request = proto::StartSendRequest {
        paths,
        expires_seconds: value
            .get("expires_seconds")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        keep_for: 0,
        keep: value.get("keep").and_then(Value::as_bool).unwrap_or(false),
    };
    match state.service.start_send(request).await {
        Ok(sent) => {
            let ticket = sent.ticket.map(|t| t.text).unwrap_or_default();
            ok(&json!({
                "ticket": { "text": ticket },
                "job_id": sent.job_id,
                "bytes": sent.bytes,
                "files": sent.files,
                "qr_svg": qr::svg(&ticket).unwrap_or_default(),
            }))
        }
        Err(e) => failed(&e),
    }
}

async fn inspect(state: &State, body: &Bytes) -> Response<Body> {
    let value = match parse(body) {
        Ok(value) => value,
        Err(refused) => return error(refused.status, refused.message),
    };
    let Some(ticket) = json::ticket(&value) else {
        return wrong_shape();
    };
    match state
        .service
        .inspect_ticket(proto::InspectTicketRequest {
            ticket: Some(ticket),
        })
        .await
    {
        Ok(offer) => ok(&json::inspected(&offer)),
        Err(e) => failed(&e),
    }
}

async fn receive(state: &State, body: &Bytes) -> Response<Body> {
    let value = match parse(body) {
        Ok(value) => value,
        Err(refused) => return error(refused.status, refused.message),
    };
    let (Some(ticket), Some(out)) = (
        json::ticket(&value),
        value.get("out").and_then(json::root_path),
    ) else {
        return wrong_shape();
    };
    let request = proto::StartReceiveRequest {
        ticket: Some(ticket),
        out: Some(out),
        force: value.get("force").and_then(Value::as_bool).unwrap_or(false),
        receipt: None,
    };
    match state.service.start_receive(request).await {
        Ok(started) => ok(&json!({ "job_id": started.job_id })),
        Err(e) => failed(&e),
    }
}

fn cancel(state: &State, id: &str) -> Response<Body> {
    match state.service.cancel_job(&proto::CancelJobRequest {
        job_id: id.to_string(),
    }) {
        Ok(answer) => ok(&json!({ "was_running": answer.was_running })),
        Err(e) => failed(&e),
    }
}

/// Every job's events as one JSON object per line, until the client goes away: the daemon never
/// closes it while it runs. A reader that falls behind reads `dropped` with the count, from the
/// service's own bounded buffer.
fn events(state: &State) -> Response<Body> {
    let mut watcher = match state.service.watch_all_events() {
        Ok(watcher) => watcher,
        Err(e) => return failed(&e),
    };
    let (tx, rx) = mpsc::channel::<Bytes>(1);
    tokio::spawn(async move {
        // A first line at once, which the page skips, so the answer's head is sent before any event.
        if tx.send(Bytes::from_static(b"\n")).await.is_err() {
            return;
        }
        loop {
            tokio::select! {
                () = tx.closed() => break,
                next = watcher.next() => {
                    let Some(event) = next else { break };
                    let mut line = json::event(&event).to_string();
                    line.push('\n');
                    if tx.send(Bytes::from(line)).await.is_err() {
                        break;
                    }
                }
            }
        }
    });
    let stream = ReceiverStream::new(rx).map(|line| Ok::<_, std::io::Error>(Frame::data(line)));
    respond(200, "application/x-ndjson", StreamBody::new(stream).boxed())
}
