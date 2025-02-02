//! Implementation of functionality related to the connection work queue that we give
//! to `handshake`.

use std::future::Future;
use std::pin::Pin;
use std::task::Poll;

use lightning_interfaces::{ConnectionWork, ConnectionWorkStealer};
use tokio::pin;

pub fn chan() -> (CommandSender, CommandStealer) {
    let (sender, receiver) = async_channel::unbounded();
    (CommandSender { sender }, CommandStealer { receiver })
}

#[derive(Clone)]
pub struct CommandSender {
    sender: async_channel::Sender<ConnectionWork>,
}

#[derive(Clone)]
pub struct CommandStealer {
    receiver: async_channel::Receiver<ConnectionWork>,
}

impl CommandSender {
    pub async fn put(&self, work: ConnectionWork) {
        self.sender
            .send(work)
            .await
            .expect("could not send through the channel.");
    }
}

impl ConnectionWorkStealer for CommandStealer {
    type AsyncFuture<'a> = StealerFuture<'a>;

    #[inline(always)]
    fn next(&mut self) -> Self::AsyncFuture<'_> {
        StealerFuture {
            fut: self.receiver.recv(),
        }
    }

    #[inline(always)]
    fn next_blocking(&mut self) -> Option<ConnectionWork> {
        self.receiver.recv_blocking().ok()
    }
}

pub struct StealerFuture<'a> {
    fut: async_channel::Recv<'a, ConnectionWork>,
}

impl<'a> Future for StealerFuture<'a> {
    type Output = Option<ConnectionWork>;

    #[inline(always)]
    fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        let fut = &mut self.fut;
        pin!(fut);
        match fut.as_mut().poll(cx) {
            Poll::Ready(value) => Poll::Ready(value.ok()),
            Poll::Pending => Poll::Pending,
        }
    }
}
