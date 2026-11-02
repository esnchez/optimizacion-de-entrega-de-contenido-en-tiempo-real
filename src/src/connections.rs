//! Counts open TCP connections per pod.
//!
//! `http_requests_in_flight` only covers a request until its response headers are sent. A viewer
//! keeps a connection open for minutes while it downloads segments every few seconds, so most of
//! the time that connection has no request in flight. This listener wrapper counts connections
//! from accept until the socket is closed, which is the number the 5M-simultaneous-connections
//! target is about and the metric the HPA scales on (`http_connections_open`).
//!
//! It also recycles old connections. A connection stays on the pod that accepted it, so after a
//! scale-up the old pods keep their viewers and the new ones only receive new connections (load
//! test B: the oldest pods held 2.2x the HPA target). Each connection gets a deadline
//! (`MAX_CONNECTION_AGE_SECS` minus up to 20% jitter); the first HTTP/1.1 response after it carries
//! `Connection: close`. hyper closes the socket once that response is written, so no request is
//! cut, and the client's next request opens a new connection that the load balancer can place on
//! any pod.

use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
    time::{Duration, Instant},
};

use axum::{
    extract::{ConnectInfo, Request, connect_info::Connected},
    http::{HeaderValue, Version, header},
    middleware::Next,
    response::Response,
    serve::{IncomingStream, Listener},
};
use metrics::Gauge;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// Wraps a listener so every accepted connection is counted while it stays open.
pub struct CountedListener<L> {
    inner: L,
    open: Gauge,
    /// `None` disables recycling.
    max_age: Option<Duration>,
    accepted: u64,
}

impl<L> CountedListener<L> {
    pub fn new(inner: L, max_age: Option<Duration>) -> Self {
        Self {
            inner,
            open: metrics::gauge!("http_connections_open"),
            max_age,
            accepted: 0,
        }
    }
}

/// Deadline after which a connection is recycled: `max_age` shortened by 0-20%, spread by the
/// connection's sequence number. Without the spread, the thousands of connections opened during
/// one ramp would all expire, and reconnect, in the same second.
fn recycle_deadline(opened: Instant, max_age: Duration, seq: u64) -> Instant {
    let jitter = (seq % 21) as f64 / 100.0;
    opened + max_age.mul_f64(1.0 - jitter)
}

impl<L: Listener> Listener for CountedListener<L> {
    type Io = CountedIo<L::Io>;
    type Addr = L::Addr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        let (io, addr) = self.inner.accept().await;
        metrics::counter!("http_connections_total").increment(1);
        self.open.increment(1.0);
        self.accepted = self.accepted.wrapping_add(1);
        let recycle_at = self
            .max_age
            .map(|age| recycle_deadline(Instant::now(), age, self.accepted));
        let io = CountedIo {
            inner: io,
            open: self.open.clone(),
            recycle_at,
        };
        (io, addr)
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        self.inner.local_addr()
    }
}

/// A connection that decrements `http_connections_open` when hyper drops it (socket closed).
pub struct CountedIo<T> {
    inner: T,
    open: Gauge,
    recycle_at: Option<Instant>,
}

/// Per-connection data that axum attaches to every request on that connection
/// (`into_make_service_with_connect_info::<ConnectionInfo>()`).
#[derive(Clone, Copy, Debug)]
pub struct ConnectionInfo {
    pub recycle_at: Option<Instant>,
}

impl<L: Listener> Connected<IncomingStream<'_, CountedListener<L>>> for ConnectionInfo {
    fn connect_info(stream: IncomingStream<'_, CountedListener<L>>) -> Self {
        Self {
            recycle_at: stream.io().recycle_at,
        }
    }
}

/// Middleware: adds `Connection: close` to the response when the connection is past its deadline.
/// Only HTTP/1.1: HTTP/1.0 closes by default, and HTTP/2 forbids the header (it would need GOAWAY).
pub async fn close_expired_connections(req: Request, next: Next) -> Response {
    let expired = req.version() == Version::HTTP_11
        && req
            .extensions()
            .get::<ConnectInfo<ConnectionInfo>>()
            .and_then(|info| info.0.recycle_at)
            .is_some_and(|deadline| Instant::now() >= deadline);
    let mut response = next.run(req).await;
    if expired {
        response
            .headers_mut()
            .insert(header::CONNECTION, HeaderValue::from_static("close"));
        metrics::counter!("http_connections_recycled_total").increment(1);
    }
    response
}

impl<T> Drop for CountedIo<T> {
    fn drop(&mut self) {
        self.open.decrement(1.0);
    }
}

// Plain delegation. `T: Unpin` (a `TcpStream`), so no pin projection is needed.
impl<T: AsyncRead + Unpin> AsyncRead for CountedIo<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for CountedIo<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write_vectored(cx, bufs)
    }

    // hyper writes headers and body chunks with one vectored write when the socket supports it.
    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use axum::{Router, body::Body, middleware, routing::get};
    use tower::ServiceExt;

    use super::*;

    #[test]
    fn deadline_is_max_age_minus_up_to_20_percent() {
        let now = Instant::now();
        let age = Duration::from_secs(100);
        for seq in 0..100 {
            let wait = recycle_deadline(now, age, seq) - now;
            assert!(
                wait <= age && wait >= Duration::from_secs(80),
                "seq {seq}: {wait:?}"
            );
        }
        assert_ne!(recycle_deadline(now, age, 1), recycle_deadline(now, age, 2));
    }

    async fn connection_header(version: Version, recycle_at: Option<Instant>) -> Option<String> {
        let app = Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(middleware::from_fn(close_expired_connections));
        let mut req = Request::builder()
            .version(version)
            .uri("/")
            .body(Body::empty())
            .unwrap();
        req.extensions_mut()
            .insert(ConnectInfo(ConnectionInfo { recycle_at }));
        let res = app.oneshot(req).await.unwrap();
        res.headers()
            .get(header::CONNECTION)
            .map(|v| v.to_str().unwrap().to_owned())
    }

    #[tokio::test]
    async fn closes_only_expired_http11_connections() {
        let past = Instant::now() - Duration::from_secs(1);
        let future = Instant::now() + Duration::from_secs(60);
        assert_eq!(
            connection_header(Version::HTTP_11, Some(past))
                .await
                .as_deref(),
            Some("close")
        );
        assert_eq!(
            connection_header(Version::HTTP_11, Some(future)).await,
            None
        );
        assert_eq!(connection_header(Version::HTTP_11, None).await, None);
        assert_eq!(connection_header(Version::HTTP_2, Some(past)).await, None);
    }
}
