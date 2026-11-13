//! What a `pre_call` hook sees of a request, and how its answer becomes the
//! request litegen goes on to authenticate, validate and handle.

use axum::body::{Body, Bytes};
use axum::extract::Request;
use axum::http::{header, request::Parts, HeaderMap, HeaderName, HeaderValue, Method, Uri};
use axum::response::{IntoResponse, Response};
use serde_json::{json, Map, Value};

use crate::api::middleware::body::BodyBudget;

/// The validator's own limit on a generation body, which `GET /v1/models`
/// serves as `capabilities.max_request_bytes`: a hook reads every body the
/// listing says the server reads (dashboard-server review #1).
pub(crate) const MAX_BODY_BYTES: usize = crate::api::middleware::body::MAX_GENERATION_BODY;

/// Headers that describe one hop, never forwarded from a hook's answer.
const HOP_BY_HOP: [&str; 7] = ["connection", "keep-alive", "proxy-connection", "transfer-encoding", "upgrade", "te", "trailer"];

/// A request held while hooks look at it.
pub(crate) struct HookedRequest {
    parts: Parts,
    content: Content,
    /// What the hooks were first shown.
    view: Value,
}

enum Content {
    /// Not read: forwarded as it arrived (the converter's upload, a GET).
    Stream(Body),
    /// Read, with what it parsed as.
    Read { bytes: Bytes, parsed: Parsed },
}

enum Parsed {
    Json,
    /// Not JSON, or a multipart litegen can't read: the validator answers it.
    Opaque,
    Multipart { boundary: String, parts: Vec<Part> },
}

struct Part {
    name: String,
    filename: Option<String>,
    content_type: Option<String>,
    data: Bytes,
}

/// Which generation routes a hook may read the body of: every POST but the
/// converter, whose upload streams through unread.
pub(crate) fn readable(method: &Method, path: &str) -> bool {
    method == Method::POST && path != "/v1/utils/models3d/convert"
}

/// Buffer the request (within the validator's limit, through `budget` and
/// its deadline) and build the view hooks see. A body read here carries its
/// charge on, so the routes behind do not charge it again.
pub(crate) async fn hook_view(req: Request, budget: &BodyBudget) -> Result<(HookedRequest, Value), Response> {
    let (mut parts, body) = req.into_parts();
    let (content, body_view, files) = if readable(&parts.method, parts.uri.path()) {
        let read = budget.read(body, MAX_BODY_BYTES).await.map_err(IntoResponse::into_response)?;
        parts.extensions.insert(read.charged());
        let bytes = read.bytes;
        let content_type = parts.headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("application/json");
        if content_type.starts_with("multipart/form-data") {
            match read_multipart(content_type, bytes.clone()).await {
                Some((boundary, mp_parts, request)) => {
                    let files: Vec<Value> = mp_parts
                        .iter()
                        .filter(|p| p.name != "request")
                        .map(|p| json!({ "name": p.name, "filename": p.filename, "content_type": p.content_type, "size": p.data.len() }))
                        .collect();
                    (Content::Read { bytes, parsed: Parsed::Multipart { boundary, parts: mp_parts } }, request, files)
                }
                None => (Content::Read { bytes, parsed: Parsed::Opaque }, Value::Null, Vec::new()),
            }
        } else {
            match serde_json::from_slice::<Value>(&bytes) {
                Ok(v) => (Content::Read { bytes, parsed: Parsed::Json }, v, Vec::new()),
                Err(_) => (Content::Read { bytes, parsed: Parsed::Opaque }, Value::Null, Vec::new()),
            }
        }
    } else {
        (Content::Stream(body), Value::Null, Vec::new())
    };
    let view = json!({
        "method": parts.method.as_str(),
        "path": parts.uri.path(),
        "query": parts.uri.query().unwrap_or(""),
        "headers": headers_view(&parts.headers),
        "body": body_view,
        "files": files,
    });
    Ok((HookedRequest { parts, content, view: view.clone() }, view))
}

/// Header names, lowercase, to their values joined with ", ". A value that
/// isn't UTF-8 is left out of the view (and kept on the request).
pub(crate) fn headers_view(headers: &HeaderMap) -> Value {
    let mut out = Map::new();
    for name in headers.keys() {
        let values: Option<Vec<&str>> = headers.get_all(name).iter().map(|v| v.to_str().ok()).collect();
        if let Some(values) = values {
            out.insert(name.as_str().to_string(), Value::String(values.join(", ")));
        }
    }
    Value::Object(out)
}

async fn read_multipart(content_type: &str, bytes: Bytes) -> Option<(String, Vec<Part>, Value)> {
    let boundary = multer::parse_boundary(content_type).ok()?;
    let stream = futures::stream::once(async move { Ok::<Bytes, std::io::Error>(bytes) });
    let mut multipart = multer::Multipart::new(stream, boundary.clone());
    let mut parts = Vec::new();
    let mut request = None;
    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_string();
        let filename = field.file_name().map(str::to_string);
        let content_type = field.content_type().map(|m| m.to_string());
        let data = field.bytes().await.ok()?;
        if name == "request" {
            request = Some(serde_json::from_slice::<Value>(&data).ok()?);
        }
        parts.push(Part { name, filename, content_type, data });
    }
    Some((boundary, parts, request?))
}

/// Check one hook's answer against the request it changes: the method and
/// path stay, headers map names to strings, and only a body litegen read as
/// JSON can change.
pub(crate) fn check_answer(hooked: &HookedRequest, answer: &Value) -> Result<(), String> {
    let original = &hooked.view;
    let Some(obj) = answer.as_object() else {
        return Err("a hook returns the request object, a string, or nothing".into());
    };
    for key in ["method", "path"] {
        if let Some(v) = obj.get(key) {
            if v != &original[key] {
                return Err(format!("a hook can't change the {key} (from {} to {v})", original[key]));
            }
        }
    }
    if let Some(q) = obj.get("query") {
        if !q.is_string() {
            return Err("query must be a string".into());
        }
    }
    if let Some(headers) = obj.get("headers") {
        let Some(map) = headers.as_object() else {
            return Err("headers must map names to strings".into());
        };
        for (name, value) in map {
            let Some(value) = value.as_str() else {
                return Err(format!("header {name} must be a string, not {value}"));
            };
            HeaderName::from_bytes(name.to_ascii_lowercase().as_bytes()).map_err(|_| format!("{name:?} isn't a valid header name"))?;
            HeaderValue::from_str(value).map_err(|_| format!("header {name}'s value isn't valid"))?;
        }
    }
    if let Some(body) = obj.get("body") {
        if body != &original["body"] {
            match &hooked.content {
                Content::Read { parsed: Parsed::Json | Parsed::Multipart { .. }, .. } => {}
                Content::Read { parsed: Parsed::Opaque, .. } => {
                    return Err("this request's body isn't JSON, so a hook can't change it".into())
                }
                Content::Stream(_) => return Err("this request's body can't be changed by a hook".into()),
            }
        }
    }
    Ok(())
}

/// The request litegen handles next. `None` gives back the identical
/// request; an answer changes its headers, query and body.
pub(crate) fn apply_answer(hooked: HookedRequest, answer: Option<Value>) -> Result<Request, String> {
    let Some(answer) = answer else {
        let HookedRequest { parts, content, .. } = hooked;
        let body = match content {
            Content::Stream(body) => body,
            Content::Read { bytes, .. } => Body::from(bytes),
        };
        return Ok(Request::from_parts(parts, body));
    };
    check_answer(&hooked, &answer)?;
    let HookedRequest { mut parts, content, view } = hooked;
    let framing = parts.headers.get(header::CONTENT_LENGTH).cloned();

    if let Some(new_headers) = answer.get("headers") {
        parts.headers = merge_headers(&parts.headers, &view["headers"], new_headers)?;
    }
    if let Some(q) = answer.get("query").and_then(Value::as_str) {
        if Some(q) != view["query"].as_str() {
            parts.uri = with_query(&parts.uri, q)?;
        }
    }
    let body_changed = answer.get("body").is_some_and(|b| b != &view["body"]);
    let body = match content {
        // A hook never sees a stream, so it can't know its length: the
        // stream keeps the framing it came with.
        Content::Stream(body) => {
            keep_framing(&mut parts.headers, framing);
            body
        }
        Content::Read { bytes, parsed } => {
            let bytes = if !body_changed {
                bytes
            } else {
                match parsed {
                    Parsed::Json => Bytes::from(serde_json::to_vec(&answer["body"]).map_err(|e| e.to_string())?),
                    Parsed::Multipart { boundary, parts: mp_parts } => rebuild_multipart(&boundary, &mp_parts, &answer["body"])?,
                    Parsed::Opaque => bytes, // refused by check_answer
                }
            };
            parts.headers.insert(header::CONTENT_LENGTH, HeaderValue::from(bytes.len()));
            Body::from(bytes)
        }
    };
    Ok(Request::from_parts(parts, body))
}

/// Put back a streamed body's own `content-length` (or its absence) after a
/// hook's header changes.
pub(crate) fn keep_framing(headers: &mut HeaderMap, framing: Option<HeaderValue>) {
    match framing {
        Some(v) => {
            headers.insert(header::CONTENT_LENGTH, v);
        }
        None => {
            headers.remove(header::CONTENT_LENGTH);
        }
    }
}

/// A header the hook left as shown keeps its original lines, byte for byte;
/// one it changed gets its new value; one it dropped is gone. A header it was
/// never shown (not UTF-8) stays unless it names that header itself.
pub(crate) fn merge_headers(original: &HeaderMap, shown: &Value, answer: &Value) -> Result<HeaderMap, String> {
    let answer = answer.as_object().ok_or("headers must map names to strings")?;
    let answer: Map<String, Value> = answer.iter().map(|(k, v)| (k.to_ascii_lowercase(), v.clone())).collect();
    let mut out = HeaderMap::new();
    for name in original.keys() {
        let key = name.as_str();
        match (answer.get(key), shown.get(key)) {
            (Some(new), Some(old)) if new == old => {
                for v in original.get_all(name) {
                    out.append(name.clone(), v.clone());
                }
            }
            (Some(_), _) => {} // changed: set below
            (None, Some(_)) => {} // shown, then dropped by the hook
            (None, None) => {
                for v in original.get_all(name) {
                    out.append(name.clone(), v.clone());
                }
            }
        }
    }
    for (key, value) in &answer {
        let unchanged = shown.get(key) == Some(value) && original.contains_key(key.as_str());
        if unchanged {
            continue;
        }
        let name = HeaderName::from_bytes(key.as_bytes()).map_err(|_| format!("{key:?} isn't a valid header name"))?;
        let value = HeaderValue::from_str(value.as_str().unwrap_or_default()).map_err(|_| format!("header {key}'s value isn't valid"))?;
        out.insert(name, value);
    }
    for hop in HOP_BY_HOP {
        out.remove(hop);
    }
    Ok(out)
}

fn with_query(uri: &Uri, query: &str) -> Result<Uri, String> {
    let path_and_query = if query.is_empty() { uri.path().to_string() } else { format!("{}?{query}", uri.path()) };
    let mut parts = uri.clone().into_parts();
    parts.path_and_query = Some(path_and_query.parse().map_err(|_| format!("{query:?} isn't a valid query"))?);
    Uri::from_parts(parts).map_err(|e| e.to_string())
}

/// The multipart again, with the new `request` part and every other part as
/// it came.
fn rebuild_multipart(boundary: &str, parts: &[Part], request: &Value) -> Result<Bytes, String> {
    let quoted = |s: &str| s.replace('"', "%22");
    let mut out = Vec::new();
    for part in parts {
        let (data, content_type) = if part.name == "request" {
            (Bytes::from(serde_json::to_vec(request).map_err(|e| e.to_string())?), Some("application/json".to_string()))
        } else {
            (part.data.clone(), part.content_type.clone())
        };
        out.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{}\"", quoted(&part.name)).as_bytes());
        if let Some(f) = &part.filename {
            out.extend_from_slice(format!("; filename=\"{}\"", quoted(f)).as_bytes());
        }
        out.extend_from_slice(b"\r\n");
        if let Some(ct) = content_type {
            out.extend_from_slice(format!("Content-Type: {ct}\r\n").as_bytes());
        }
        out.extend_from_slice(b"\r\n");
        out.extend_from_slice(&data);
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    Ok(Bytes::from(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn awkward_request() -> Request {
        Request::post("/v1/images/generations?a=1")
            .header("content-type", "application/json")
            .header("x-raw", HeaderValue::from_bytes(&[0xff, 0xfe, b'a']).unwrap())
            .header("x-dup", "1")
            .header("x-dup", "2")
            .header("cookie", "a=1; b=2")
            .body(Body::from(r#"{"prompt":"a fox"}"#))
            .unwrap()
    }

    fn header_lines(h: &HeaderMap) -> Vec<(String, Vec<u8>)> {
        h.iter().map(|(k, v)| (k.as_str().to_string(), v.as_bytes().to_vec())).collect()
    }

    /// Room for any body these tests send.
    fn ample() -> BodyBudget {
        BodyBudget::new(4, MAX_BODY_BYTES, std::time::Duration::from_secs(60))
    }

    /// Dashboard-server review #1: a hook reads every body the listing says
    /// the server reads (`capabilities.max_request_bytes`, the generation
    /// routes' `MAX_GENERATION_BODY`), and refuses one byte more as they do.
    /// A hook layer with a lower limit of its own would refuse, with hooks
    /// configured, a request the served limit says is fine.
    #[tokio::test]
    async fn a_hooked_body_may_be_as_large_as_the_served_limit() {
        let max = crate::api::middleware::body::MAX_GENERATION_BODY;
        let body_of = |len: usize| {
            let mut b = br#"{"model":"mock/image-gen","prompt":"a red fox""#.to_vec();
            b.resize(len - 1, b' ');
            b.push(b'}');
            Request::post("/v1/images/generations").header("content-type", "application/json").body(Body::from(b)).unwrap()
        };
        let (_, view) = hook_view(body_of(max), &ample()).await.unwrap_or_else(|r| panic!("refused a body of the served size: {}", r.status()));
        assert_eq!(view["body"]["prompt"], "a red fox");
        let refused = hook_view(body_of(max + 1), &ample()).await.err().map(|r| r.status());
        assert_eq!(refused, Some(axum::http::StatusCode::PAYLOAD_TOO_LARGE));
    }

    #[tokio::test]
    async fn an_unchanged_answer_rebuilds_the_identical_request() {
        let original = awkward_request();
        let (want_method, want_uri, want_headers) = (original.method().clone(), original.uri().clone(), header_lines(original.headers()));
        let (hooked, _) = hook_view(original, &ample()).await.unwrap();
        let rebuilt = apply_answer(hooked, None).unwrap();
        assert_eq!(rebuilt.method(), want_method);
        assert_eq!(rebuilt.uri(), &want_uri);
        assert_eq!(header_lines(rebuilt.headers()), want_headers);
        let body = axum::body::to_bytes(rebuilt.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body[..], br#"{"prompt":"a fox"}"#);
    }

    #[tokio::test]
    async fn the_view_joins_repeated_headers_and_leaves_out_values_that_arent_utf8() {
        let (_, view) = hook_view(awkward_request(), &ample()).await.unwrap();
        assert_eq!(view["headers"]["x-dup"], "1, 2");
        assert_eq!(view["headers"]["cookie"], "a=1; b=2");
        assert!(view["headers"].get("x-raw").is_none(), "{view}");
        assert_eq!(view["query"], "a=1");
        assert_eq!(view["body"]["prompt"], "a fox");
    }

    #[tokio::test]
    async fn a_changed_body_keeps_every_header_the_hook_left_alone_byte_for_byte() {
        let (hooked, mut view) = hook_view(awkward_request(), &ample()).await.unwrap();
        view["body"]["prompt"] = json!("a fox (hooked)");
        view["headers"]["authorization"] = json!("Bearer k");
        let rebuilt = apply_answer(hooked, Some(view)).unwrap();
        let h = rebuilt.headers();
        assert_eq!(h.get_all("x-dup").iter().map(|v| v.as_bytes()).collect::<Vec<_>>(), [b"1".as_slice(), b"2"]);
        assert_eq!(h.get("x-raw").unwrap().as_bytes(), &[0xff, 0xfe, b'a']);
        assert_eq!(h.get("authorization").unwrap(), "Bearer k");
        let body = axum::body::to_bytes(rebuilt.into_body(), usize::MAX).await.unwrap();
        assert_eq!(serde_json::from_slice::<Value>(&body).unwrap()["prompt"], "a fox (hooked)");
    }

    #[tokio::test]
    async fn a_header_the_hook_removed_is_gone_and_hop_by_hop_headers_never_pass() {
        let (hooked, mut view) = hook_view(awkward_request(), &ample()).await.unwrap();
        view["headers"].as_object_mut().unwrap().remove("cookie");
        view["headers"]["connection"] = json!("close");
        let rebuilt = apply_answer(hooked, Some(view)).unwrap();
        assert!(rebuilt.headers().get("cookie").is_none());
        assert!(rebuilt.headers().get("connection").is_none());
    }

    #[tokio::test]
    async fn a_streamed_body_keeps_its_own_length_whatever_the_hook_says() {
        let converter = || {
            Request::post("/v1/utils/models3d/convert?to=obj")
                .header("content-type", "model/gltf-binary")
                .header("content-length", "10")
                .body(Body::from("0123456789"))
                .unwrap()
        };
        let (hooked, mut view) = hook_view(converter(), &ample()).await.unwrap();
        view["headers"]["content-length"] = json!("99");
        view["headers"]["x-note"] = json!("seen");
        let rebuilt = apply_answer(hooked, Some(view)).unwrap();
        assert_eq!(rebuilt.headers().get("content-length").unwrap(), "10", "the stream's framing is its own");
        assert_eq!(rebuilt.headers().get("x-note").unwrap(), "seen", "other changes still apply");

        let (hooked, mut view) = hook_view(converter(), &ample()).await.unwrap();
        view["headers"].as_object_mut().unwrap().remove("content-length");
        let rebuilt = apply_answer(hooked, Some(view)).unwrap();
        assert_eq!(rebuilt.headers().get("content-length").unwrap(), "10");
    }
}
