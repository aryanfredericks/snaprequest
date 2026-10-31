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
}

/// Shared handle to a connection's measurements.
#[derive(Debug, Default, Clone)]
pub(crate) struct Meter(Arc<Mutex<MeterState>>);

impl Meter {
    fn lock(&self) -> MutexGuard<'_, MeterState> {
        // A poisoned lock only means a panic elsewhere; the counters are still usable.
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Clears the counters before a new request is sent on this connection.
    pub(crate) fn reset(&self) {
        *self.lock() = MeterState::default();
    }

    pub(crate) fn snapshot(&self) -> MeterSnapshot {
        self.lock().snap
    }

    fn on_write(&self, n: usize) {
        let mut s = self.lock();
        s.snap.bytes_written += n as u64;
        s.snap.last_write_at = Some(Instant::now());
    }

    fn on_read(&self, n: usize) {
        let mut s = self.lock();
        if s.snap.first_read_at.is_none() {
            s.snap.first_read_at = Some(Instant::now());
            s.snap.write_done_at = s.snap.last_write_at;
        }
        s.snap.bytes_read += n as u64;
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
        let n = buf.filled().len() - before;
        if n > 0 {
            self.meter.on_read(n);
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
            self.meter.on_write(n);
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
            self.meter.on_write(n);
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

        meter.reset();
        assert_eq!(meter.snapshot().bytes_read, 0);
        assert!(meter.snapshot().first_read_at.is_none());
    }
}
