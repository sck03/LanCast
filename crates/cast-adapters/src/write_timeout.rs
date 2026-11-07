//! Bound a stalled transport write without limiting a stream's total lifetime.
use std::{
    future::Future,
    io,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    time::{Instant, Sleep},
};

pub(crate) struct WriteTimeout<S> {
    stream: S,
    timeout: Duration,
    timer: Pin<Box<Sleep>>,
    waiting: bool,
}

impl<S: Unpin> WriteTimeout<S> {
    pub(crate) fn new(stream: S, timeout: Duration) -> Self {
        Self {
            stream,
            timeout,
            timer: Box::pin(tokio::time::sleep(timeout)),
            waiting: false,
        }
    }

    fn poll_io<T>(
        &mut self,
        cx: &mut Context<'_>,
        operation: impl FnOnce(Pin<&mut S>, &mut Context<'_>) -> Poll<io::Result<T>>,
    ) -> Poll<io::Result<T>> {
        // Check an existing deadline first: a delayed executor must not revive
        // an expired write just because the transport has become ready.
        if self.waiting && self.timer.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "HTTP_WRITE_TIMEOUT",
            )));
        }
        let result = operation(Pin::new(&mut self.stream), cx);
        if result.is_ready() {
            self.waiting = false;
        } else if !self.waiting {
            self.waiting = true;
            self.timer.as_mut().reset(Instant::now() + self.timeout);
            let _ = self.timer.as_mut().poll(cx);
        }
        result
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for WriteTimeout<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for WriteTimeout<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.get_mut()
            .poll_io(cx, |stream, cx| stream.poll_write(cx, buf))
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        self.get_mut()
            .poll_io(cx, |stream, cx| stream.poll_write_vectored(cx, bufs))
    }

    fn is_write_vectored(&self) -> bool {
        self.stream.is_write_vectored()
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.get_mut()
            .poll_io(cx, |stream, cx| stream.poll_flush(cx))
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.get_mut()
            .poll_io(cx, |stream, cx| stream.poll_shutdown(cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test(start_paused = true)]
    async fn progress_renews_budget_and_idle_source_does_not_expire() {
        let (writer, mut reader) = tokio::io::duplex(1);
        let mut writer = WriteTimeout::new(writer, Duration::from_secs(5));
        for byte in 0..4 {
            let data = [byte];
            writer.write_all(&data).await.unwrap();
            let mut pending = Box::pin(writer.write_all(&data));
            assert!(futures_util::poll!(&mut pending).is_pending());
            tokio::time::advance(Duration::from_secs(4)).await;
            assert_eq!(reader.read_u8().await.unwrap(), byte);
            pending.await.unwrap();
            assert_eq!(reader.read_u8().await.unwrap(), byte);
            // No pending write: an idle producer has its own body policy.
            tokio::time::advance(Duration::from_secs(10)).await;
        }
    }

    #[tokio::test(start_paused = true)]
    async fn deadline_rejects_late_readiness_and_vectored_writes() {
        let (writer, mut reader) = tokio::io::duplex(1);
        let mut writer = WriteTimeout::new(writer, Duration::from_secs(5));
        writer.write_all(b"a").await.unwrap();
        let buffers = [io::IoSlice::new(b"b")];
        let mut pending = Box::pin(writer.write_vectored(&buffers));
        assert!(futures_util::poll!(&mut pending).is_pending());
        tokio::time::advance(Duration::from_secs(5)).await;
        assert_eq!(reader.read_u8().await.unwrap(), b'a');
        assert_eq!(pending.await.unwrap_err().kind(), io::ErrorKind::TimedOut);
    }
}
