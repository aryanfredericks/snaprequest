//! An I/O wrapper that measures what actually crosses the socket.
//!
//! hyper only tells us when the response *headers* are parsed. To report real
//! time-to-first-byte, send time and on-the-wire sizes we watch the socket itself.

use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll};
use std::time::Instant;

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// Measurements for the current exchange on one connection. Reset before each request.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct MeterSnapshot {
    pub bytes_written: u64,
    pub bytes_read: u64,
    /// When the first response byte was read.
    pub first_read_at: Option<Instant>,
    /// When the last request byte was written before the first response byte arrived.
    pub write_done_at: Option<Instant>,
    /// When the most recent write happened.
    pub last_write_at: Option<Instant>,
}

#[derive(Debug, Default)]
struct MeterState {
    snap: MeterSnapshot,
    capture: Option<Capture>,
}

/// Bytes of the current exchange, each direction capped at `limit`.
#[derive(Debug, Default)]
struct Capture {
    limit: usize,
    sent: Vec<u8>,
    received: Vec<u8>,
    truncated: bool,
}

impl Capture {
    fn push(limit: usize, buf: &mut Vec<u8>, truncated: &mut bool, data: &[u8]) {
        let room = limit.saturating_sub(buf.len());
        if data.len() > room {
            *truncated = true;
        }
        buf.extend_from_slice(&data[..data.len().min(room)]);
    }
}

/// Captured bytes of one exchange.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Captured {
    pub sent: bytes::Bytes,
    pub received: bytes::Bytes,
    pub truncated: bool,
}

/// Shared handle to a connection's measurements.
#[derive(Debug, Default, Clone)]
pub(crate) struct Meter(Arc<Mutex<MeterState>>);

impl Meter {
    fn lock(&self) -> MutexGuard<'_, MeterState> {
        // A poisoned lock only means a panic elsewhere; the counters are still usable.
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Clears the counters before a new request is sent on this connection, and
    /// starts capturing bytes (up to `capture_limit` per direction) if given.
    pub(crate) fn reset(&self, capture_limit: Option<usize>) {
        *self.lock() = MeterState {
            snap: MeterSnapshot::default(),
            capture: capture_limit.map(|limit| Capture {
                limit,
                ..Capture::default()
            }),
        };
    }

    /// Takes the captured bytes, if capturing was on.
    pub(crate) fn take_capture(&self) -> Option<Captured> {
        self.lock().capture.take().map(|c| Captured {
            sent: c.sent.into(),
            received: c.received.into(),
            truncated: c.truncated,
        })
    }

    pub(crate) fn snapshot(&self) -> MeterSnapshot {
        self.lock().snap
    }

    fn on_write(&self, data: &[u8]) {
        let mut s = self.lock();
        s.snap.bytes_written += data.len() as u64;
        s.snap.last_write_at = Some(Instant::now());
        if let Some(c) = &mut s.capture {
            Capture::push(c.limit, &mut c.sent, &mut c.truncated, data);
        }
    }

    fn on_read(&self, data: &[u8]) {
        let mut s = self.lock();
        if s.snap.first_read_at.is_none() {
            s.snap.first_read_at = Some(Instant::now());
            s.snap.write_done_at = s.snap.last_write_at;
        }
        s.snap.bytes_read += data.len() as u64;
        if let Some(c) = &mut s.capture {
            Capture::push(c.limit, &mut c.received, &mut c.truncated, data);
        }
    }
}

/// Wraps a socket and reports every read and write to a [`Meter`].
#[derive(Debug)]
pub(crate) struct MeteredIo<T> {
    inner: T,
    meter: Meter,
}

impl<T> MeteredIo<T> {
    pub(crate) fn new(inner: T, meter: Meter) -> Self {
        Self { inner, meter }
    }
}

impl<T: AsyncRead + Unpin> AsyncRead for MeteredIo<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        let res = Pin::new(&mut self.inner).poll_read(cx, buf);
        let filled = &buf.filled()[before..];
        if !filled.is_empty() {
            self.meter.on_read(filled);
        }
        res
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for MeteredIo<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let res = Pin::new(&mut self.inner).poll_write(cx, buf);
        if let Poll::Ready(Ok(n)) = res
            && n > 0
        {
            self.meter.on_write(&buf[..n]);
        }
        res
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let res = Pin::new(&mut self.inner).poll_write_vectored(cx, bufs);
        if let Poll::Ready(Ok(n)) = res
            && n > 0
        {
            // Report exactly the first `n` bytes across the slices.
            let mut left = n;
            for b in bufs {
                if left == 0 {
                    break;
                }
                let take = left.min(b.len());
                self.meter.on_write(&b[..take]);
                left -= take;
            }
        }
        res
    }

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
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn captures_bytes_up_to_the_limit() {
        let (client, mut server) = tokio::io::duplex(64);
        let meter = Meter::default();
        meter.reset(Some(4));
        let mut io = MeteredIo::new(client, meter.clone());

        io.write_all(b"abc").await.unwrap();
        server.write_all(b"xyz123").await.unwrap();
        let mut buf = [0u8; 6];
        io.read_exact(&mut buf).await.unwrap();

        let c = meter.take_capture().unwrap();
        assert_eq!(&c.sent[..], b"abc");
        assert_eq!(&c.received[..], b"xyz1");
        assert!(c.truncated);
        assert!(meter.take_capture().is_none());
    }

    #[tokio::test]
    async fn counts_bytes_and_marks_first_read() {
        let (client, mut server) = tokio::io::duplex(64);
        let meter = Meter::default();
        let mut io = MeteredIo::new(client, meter.clone());

        io.write_all(b"hello").await.unwrap();
        let mut buf = [0u8; 5];
        server.read_exact(&mut buf).await.unwrap();
        assert!(meter.snapshot().first_read_at.is_none());

        server.write_all(b"world!").await.unwrap();
        let mut buf = [0u8; 6];
        io.read_exact(&mut buf).await.unwrap();

        let snap = meter.snapshot();
        assert_eq!(snap.bytes_written, 5);
        assert_eq!(snap.bytes_read, 6);
        let (done, first) = (snap.write_done_at.unwrap(), snap.first_read_at.unwrap());
        assert!(done <= first);

        meter.reset(None);
        assert_eq!(meter.snapshot().bytes_read, 0);
        assert!(meter.snapshot().first_read_at.is_none());
    }
}
