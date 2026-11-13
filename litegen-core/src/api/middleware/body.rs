//! Request bodies, read within a budget of bytes and a deadline.
//!
//! A generation slot (`backpressure::InFlightLimit`) used to be taken before
//! the body was read, so a client that sent its body slowly, or not at all,
//! held a slot for as long as it liked: one such client per slot answered
//! everyone else 503. Bodies are now read first, each byte charged to a
//! budget as it arrives, and only a body that is all there takes a slot. The
//! budget is what the slots used to bound, `max_in_flight` × 25 MiB, so
//! memory stays bounded as before, but a client that has sent 10 bytes holds
//! 1 KiB, not a slot. Each body has a deadline too (408 `body_timeout`).
//!
//! A body is charged once. A layer that reads it before the handler (the
//! hook layer, the idempotency middleware) hands the request on carrying
//! [`BodyCharged`], and a later reader takes the bytes from memory without
//! charging them again. The charge lasts until the bytes and every request
//! carrying them are gone. A hook that replaces a body with a larger one is
//! not charged for the growth.

use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, Bytes, HttpBody as _};
use axum::extract::Request;
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use futures::StreamExt as _;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// The most a generation, quote or segmentation body may be.
pub const MAX_GENERATION_BODY: usize = 25 * 1024 * 1024;

/// The budget's unit: one permit is one KiB of body.
const KIB: usize = 1024;

/// The bytes request bodies may hold in memory at once, and how long one body
/// may take to arrive.
pub struct BodyBudget {
    kib: Arc<Semaphore>,
    timeout: Duration,
}

/// What a read body holds of its budget, until the bytes and every request
/// carrying them are gone. A request that carries one has had its body read
/// and charged; a later reader takes it from memory without charging again.
#[derive(Clone)]
pub struct BodyCharged {
    /// Never read: held so the charge lasts as long as any holder.
    _held: Arc<Option<OwnedSemaphorePermit>>,
}

/// A body read in full, with its share of the budget.
pub struct Buffered {
    pub bytes: Bytes,
    charged: BodyCharged,
}

impl std::fmt::Debug for Buffered {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Buffered({} bytes)", self.bytes.len())
    }
}

/// Why a body was not read.
#[derive(Debug)]
pub enum BodyError {
    /// Over the limit, as declared or as sent: 413 `body_too_large`.
    TooLarge { limit: usize },
    /// Not all there within the deadline: 408 `body_timeout`.
    TimedOut(Duration),
    /// The budget is spent: 503 `server_at_capacity`, with `Retry-After`.
    Busy,
    /// It broke off, or its framing was bad: 400 `body_unreadable`.
    Unreadable(String),
}

impl BodyError {
    pub fn status(&self) -> StatusCode {
        match self {
            BodyError::TooLarge { .. } => StatusCode::PAYLOAD_TOO_LARGE,
            BodyError::TimedOut(_) => StatusCode::REQUEST_TIMEOUT,
            BodyError::Busy => StatusCode::SERVICE_UNAVAILABLE,
            BodyError::Unreadable(_) => StatusCode::BAD_REQUEST,
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            BodyError::TooLarge { .. } => "body_too_large",
            BodyError::TimedOut(_) => "body_timeout",
            BodyError::Busy => "server_at_capacity",
            BodyError::Unreadable(_) => "body_unreadable",
        }
    }

    pub fn message(&self) -> String {
        match self {
            BodyError::TooLarge { limit } => format!("body is over the {limit}-byte limit"),
            BodyError::TimedOut(d) => format!("the request body did not arrive within {} s", d.as_secs_f64()),
            BodyError::Busy => "server at capacity, retry shortly".to_string(),
            BodyError::Unreadable(why) => format!("the request body could not be read: {why}"),
        }
    }
}

/// The API's error shape (`validation_error`), with `Retry-After: 1` on a 503.
impl IntoResponse for BodyError {
    fn into_response(self) -> Response {
        let body = crate::api::middleware::validator::err_body(self.code(), &self.message(), None, "");
        let mut resp = (self.status(), axum::Json(body)).into_response();
        if matches!(self, BodyError::Busy) {
            resp.headers_mut().insert(axum::http::header::RETRY_AFTER, axum::http::HeaderValue::from_static("1"));
        }
        resp
    }
}

impl BodyBudget {
    /// Room for `bodies` bodies of up to `per_body` bytes each, each to arrive
    /// within `timeout`. At least one body always fits; a budget past what a
    /// semaphore can count is clamped to the most it can.
    pub fn new(bodies: usize, per_body: usize, timeout: Duration) -> Self {
        let kib = bodies.max(1).saturating_mul(per_body.div_ceil(KIB)).min(Semaphore::MAX_PERMITS);
        Self { kib: Arc::new(Semaphore::new(kib)), timeout }
    }

    /// How long one body may take to arrive.
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Read `body` in full, up to `limit` bytes, within the deadline, charging
    /// each byte to the budget as it arrives. A body that declares more than
    /// `limit` is refused before a byte is read.
    pub async fn read(&self, body: Body, limit: usize) -> Result<Buffered, BodyError> {
        if body.size_hint().lower() > limit as u64 {
            return Err(BodyError::TooLarge { limit });
        }
        let read = async move {
            let mut stream = body.into_data_stream();
            let mut chunks: Vec<Bytes> = Vec::new();
            let mut total = 0usize;
            let mut held: Option<OwnedSemaphorePermit> = None;
            let mut held_kib = 0usize;
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|e| BodyError::Unreadable(e.to_string()))?;
                total = total.saturating_add(chunk.len());
                if total > limit {
                    return Err(BodyError::TooLarge { limit });
                }
                let needed = total.div_ceil(KIB);
                if needed > held_kib {
                    // `needed` is at most `limit` in KiB, which fits a u32.
                    let more = u32::try_from(needed - held_kib).map_err(|_| BodyError::TooLarge { limit })?;
                    let charge = self.kib.clone().try_acquire_many_owned(more).map_err(|_| BodyError::Busy)?;
                    match held.as_mut() {
                        Some(permit) => permit.merge(charge),
                        None => held = Some(charge),
                    }
                    held_kib = needed;
                }
                chunks.push(chunk);
            }
            let bytes = match chunks.len() {
                0 => Bytes::new(),
                1 => chunks.pop().unwrap_or_default(),
                _ => Bytes::from(chunks.concat()),
            };
            Ok(Buffered { bytes, charged: BodyCharged { _held: Arc::new(held) } })
        };
        match tokio::time::timeout(self.timeout, read).await {
            Ok(read) => read,
            // What had arrived goes back to the budget with the dropped read.
            Err(_) => Err(BodyError::TimedOut(self.timeout)),
        }
    }

    /// Read `req`'s body in full, up to `limit` bytes. One that an earlier
    /// reader charged (it carries [`BodyCharged`]) is in memory already: it is
    /// read without charging again, and the bytes keep that charge.
    pub async fn read_request(&self, req: Request, limit: usize) -> Result<(Parts, Buffered), BodyError> {
        let (parts, body) = req.into_parts();
        let read = match parts.extensions.get::<BodyCharged>().cloned() {
            Some(charged) => {
                let bytes = axum::body::to_bytes(body, limit).await.map_err(|_| BodyError::TooLarge { limit })?;
                Buffered { bytes, charged }
            }
            None => self.read(body, limit).await?,
        };
        Ok((parts, read))
    }
}

impl Buffered {
    /// Its share of the budget, for a request that carries these bytes on.
    pub fn charged(&self) -> BodyCharged {
        self.charged.clone()
    }

    /// The request again, carrying these bytes and their charge.
    pub fn into_request(self, mut parts: Parts) -> Request {
        parts.extensions.insert(self.charged);
        Request::from_parts(parts, Body::from(self.bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::task::{Context, Poll};

    use futures::StreamExt;
    use hyper::body::{Frame, SizeHint};

    const MIB: usize = 1024 * 1024;
    const LONG: Duration = Duration::from_secs(60);

    /// A body that sends `first`, and then nothing more, ever.
    fn stalled(first: Bytes) -> Body {
        Body::from_stream(futures::stream::once(async move { Ok::<_, std::io::Error>(first) }).chain(futures::stream::pending()))
    }

    /// A body that says it is `declared` bytes long, sends `first` and ends
    /// (or stalls, when `then_stall`). `polled` says whether it was read at all.
    struct Declared {
        declared: u64,
        first: Option<Bytes>,
        then_stall: bool,
        polled: Arc<AtomicBool>,
    }

    impl hyper::body::Body for Declared {
        type Data = Bytes;
        type Error = std::io::Error;

        fn poll_frame(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Bytes>, std::io::Error>>> {
            self.polled.store(true, Ordering::SeqCst);
            match self.first.take() {
                Some(bytes) => Poll::Ready(Some(Ok(Frame::data(bytes)))),
                None if self.then_stall => Poll::Pending,
                None => Poll::Ready(None),
            }
        }

        fn size_hint(&self) -> SizeHint {
            SizeHint::with_exact(self.declared)
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_read_that_stalls_past_its_deadline_is_408_and_gives_back_its_bytes() {
        let budget = BodyBudget::new(1, 20 * MIB, Duration::from_secs(2));
        let read = tokio::time::timeout(LONG, budget.read(stalled(Bytes::from(vec![b'x'; 10 * MIB])), 20 * MIB)).await;
        let Ok(read) = read else { panic!("the stalled read was never answered") };
        assert!(matches!(read, Err(BodyError::TimedOut(d)) if d == Duration::from_secs(2)), "{read:?}");
        let again = budget.read(Body::from(vec![b'y'; 20 * MIB]), 20 * MIB).await;
        assert!(again.is_ok(), "the 10 MiB that arrived went back to the budget: {again:?}");
    }

    /// A guard: green before and after the budget, since a body that has
    /// sent 10 bytes must never stand for the 25 MiB it declared.
    #[tokio::test]
    async fn the_budget_counts_the_bytes_that_arrived_not_the_declared_length() {
        let budget = Arc::new(BodyBudget::new(1, MAX_GENERATION_BODY, LONG));
        let polled = Arc::new(AtomicBool::new(false));
        let declared = Declared { declared: MAX_GENERATION_BODY as u64, first: Some(Bytes::from_static(b"0123456789")), then_stall: true, polled: polled.clone() };
        let stalled_read = {
            let budget = budget.clone();
            tokio::spawn(async move { budget.read(Body::new(declared), MAX_GENERATION_BODY).await })
        };
        while !polled.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
        let second = budget.read(Body::from(vec![b'x'; 20 * MIB]), MAX_GENERATION_BODY).await;
        stalled_read.abort();
        assert!(second.is_ok(), "{second:?}");
    }

    #[tokio::test]
    async fn a_body_declared_over_the_limit_is_413_before_a_byte_is_read() {
        let budget = BodyBudget::new(1, MAX_GENERATION_BODY, LONG);
        let polled = Arc::new(AtomicBool::new(false));
        let declared = Declared { declared: MAX_GENERATION_BODY as u64 + 1, first: Some(Bytes::from_static(b"x")), then_stall: false, polled: polled.clone() };
        let read = budget.read(Body::new(declared), MAX_GENERATION_BODY).await;
        assert!(matches!(read, Err(BodyError::TooLarge { limit: MAX_GENERATION_BODY })), "{read:?}");
        assert!(!polled.load(Ordering::SeqCst), "the body was read");
    }

    #[tokio::test]
    async fn when_the_budget_is_spent_a_read_answers_503() {
        let budget = BodyBudget::new(1, MAX_GENERATION_BODY, LONG);
        let first = budget.read(Body::from(vec![b'x'; 20 * MIB]), MAX_GENERATION_BODY).await.expect("the first body fits");
        let second = budget.read(Body::from(vec![b'y'; 20 * MIB]), MAX_GENERATION_BODY).await;
        assert!(matches!(second, Err(BodyError::Busy)), "{second:?}");
        drop(first);
        let third = budget.read(Body::from(vec![b'z'; 20 * MIB]), MAX_GENERATION_BODY).await;
        assert!(third.is_ok(), "the first body's bytes went back to the budget: {third:?}");
    }

    #[tokio::test]
    async fn a_body_that_breaks_off_is_400_body_unreadable() {
        let budget = BodyBudget::new(1, MAX_GENERATION_BODY, LONG);
        let chunks = vec![
            Ok(Bytes::from_static(b"{\"model\":")),
            Err(std::io::Error::new(std::io::ErrorKind::ConnectionReset, "the client went away")),
        ];
        let read = budget.read(Body::from_stream(futures::stream::iter(chunks)), MAX_GENERATION_BODY).await;
        assert!(matches!(&read, Err(BodyError::Unreadable(why)) if why.contains("the client went away")), "{read:?}");
    }

    /// A body a layer read and handed on (the hook layer, the idempotency
    /// middleware) is charged once, and stays charged until the last holder of
    /// its bytes is gone.
    #[tokio::test]
    async fn a_request_read_twice_is_charged_once_until_its_bytes_are_gone() {
        let budget = BodyBudget::new(1, MAX_GENERATION_BODY, LONG);
        let req = Request::post("/v1/images/generations").body(Body::from(vec![b'x'; 20 * MIB])).unwrap();
        let (parts, first) = budget.read_request(req, MAX_GENERATION_BODY).await.unwrap();
        let handed_on = first.into_request(parts);
        let (_, again) = budget.read_request(handed_on, MAX_GENERATION_BODY).await.expect("read from memory, not charged again");
        assert_eq!(again.bytes.len(), 20 * MIB);
        let other = budget.read(Body::from(vec![b'y'; 20 * MIB]), MAX_GENERATION_BODY).await;
        assert!(matches!(other, Err(BodyError::Busy)), "the bytes handed on are still charged: {other:?}");
        drop(again);
        let other = budget.read(Body::from(vec![b'y'; 20 * MIB]), MAX_GENERATION_BODY).await;
        assert!(other.is_ok(), "and given back once they are gone: {other:?}");
    }

    /// A guard: a `max_in_flight` far past what a semaphore can count is a
    /// budget of the most it can, never a panic at startup.
    #[test]
    fn a_budget_past_what_a_semaphore_holds_is_clamped() {
        let _ = BodyBudget::new(usize::MAX, MAX_GENERATION_BODY, LONG);
        let _ = BodyBudget::new(0, MAX_GENERATION_BODY, LONG);
    }
}
