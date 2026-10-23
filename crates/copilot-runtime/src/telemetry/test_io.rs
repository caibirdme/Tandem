//! Deterministic control of an EOF read completed before the producer shuts down.

use std::{
    future::Future,
    io::{self, SeekFrom},
    pin::Pin,
    task::{Context, Poll},
};

use tokio::{
    io::{AsyncRead, AsyncSeek, AsyncWrite, ReadBuf},
    sync::oneshot,
};

pub(super) struct GatedEofFile {
    file: tokio::fs::File,
    entered: Option<oneshot::Sender<()>>,
    release: oneshot::Receiver<()>,
    held: bool,
}

impl GatedEofFile {
    pub(super) fn new(file: tokio::fs::File) -> (Self, oneshot::Receiver<()>, oneshot::Sender<()>) {
        let (entered, reached) = oneshot::channel();
        let (release, released) = oneshot::channel();
        (
            Self {
                file,
                entered: Some(entered),
                release: released,
                held: false,
            },
            reached,
            release,
        )
    }

    fn poll_held_eof(&mut self, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        match Pin::new(&mut self.release).poll(context) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(result) => {
                self.held = false;
                Poll::Ready(result.map_err(|_| io::Error::other("the EOF gate was abandoned")))
            }
        }
    }
}

impl AsyncRead for GatedEofFile {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        destination: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.held {
            return this.poll_held_eof(context);
        }
        let filled = destination.filled().len();
        match Pin::new(&mut this.file).poll_read(context, destination) {
            Poll::Ready(Ok(())) if filled == destination.filled().len() => {
                if let Some(entered) = this.entered.take() {
                    this.held = true;
                    let _ = entered.send(());
                    this.poll_held_eof(context)
                } else {
                    Poll::Ready(Ok(()))
                }
            }
            result => result,
        }
    }
}

impl AsyncSeek for GatedEofFile {
    fn start_seek(self: Pin<&mut Self>, position: SeekFrom) -> io::Result<()> {
        Pin::new(&mut self.get_mut().file).start_seek(position)
    }

    fn poll_complete(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<u64>> {
        Pin::new(&mut self.get_mut().file).poll_complete(context)
    }
}

impl AsyncWrite for GatedEofFile {
    fn poll_write(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().file).poll_write(context, buffer)
    }

    fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().file).poll_flush(context)
    }

    fn poll_shutdown(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().file).poll_shutdown(context)
    }
}
