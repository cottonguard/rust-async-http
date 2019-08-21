use crate::reactor;
use futures::prelude::*;
use log::*;
use mio::*;
use std::io::{self, prelude::*};
use std::net::SocketAddr;
use std::pin::Pin;
use std::task;

pub struct TcpListener {
    listener: mio::net::TcpListener,
    reactor: reactor::ReactorHandle,
}

impl TcpListener {
    pub fn bind(addr: &SocketAddr) -> io::Result<TcpListener> {
        let listener = mio::net::TcpListener::bind(addr)?;
        let tcp = TcpListener {
            reactor: reactor::register(&listener, Ready::readable())?,
            listener,
        };
        Ok(tcp)
    }

    pub async fn accept(&self) -> io::Result<(TcpStream, SocketAddr)> {
        futures::future::poll_fn(|cx| self.poll_accept(cx)).await
    }

    pub fn poll_accept(
        &self,
        cx: &mut task::Context,
    ) -> task::Poll<io::Result<(TcpStream, SocketAddr)>> {
        if self.reactor.readiness().is_readable() {
            match self.listener.accept() {
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                    self.reactor.remove_readiness(Ready::readable());
                    self.reactor.set_read_waker(cx.waker().clone());
                    task::Poll::Pending
                }
                Ok((sock, addr)) => task::Poll::Ready(Ok((TcpStream::from_mio(sock)?, addr))),
                Err(e) => task::Poll::Ready(Err(e)),
            }
        } else {
            self.reactor.set_read_waker(cx.waker().clone());
            task::Poll::Pending
        }
    }
}

#[derive(Debug)]
pub struct TcpStream {
    sock: mio::net::TcpStream,
    reactor: reactor::ReactorHandle,
}

impl TcpStream {
    pub fn from_mio(sock: mio::net::TcpStream) -> io::Result<TcpStream> {
        let tcp = TcpStream {
            reactor: reactor::register(&sock, Ready::readable() | Ready::writable())?,
            sock,
        };
        Ok(tcp)
    }

    pub fn peer_addr(&self) -> io::Result<std::net::SocketAddr> {
        self.sock.peer_addr()
    }
}

impl AsyncRead for TcpStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut task::Context,
        buf: &mut [u8],
    ) -> task::Poll<io::Result<usize>> {
        trace!("poll_read");
        if self.reactor.readiness().is_readable() {
            match self.sock.read(buf) {
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                    self.reactor.remove_readiness(Ready::readable());
                    self.reactor.set_read_waker(cx.waker().clone());
                    task::Poll::Pending
                }
                res => {
                    self.reactor.reset_read_waker();
                    task::Poll::Ready(res)
                }
            }
        } else {
            self.reactor.set_read_waker(cx.waker().clone());
            task::Poll::Pending
        }
    }
}

impl AsyncWrite for TcpStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut task::Context,
        buf: &[u8],
    ) -> task::Poll<io::Result<usize>> {
        trace!("poll_write ({})", buf.len());
        if self.reactor.readiness().is_writable() {
            match self.sock.write(buf) {
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                    self.reactor.remove_readiness(Ready::writable());
                    self.reactor.set_write_waker(cx.waker().clone());
                    task::Poll::Pending
                }
                res => {
                    self.reactor.reset_write_waker();
                    task::Poll::Ready(res)
                }
            }
        } else {
            self.reactor.set_write_waker(cx.waker().clone());
            task::Poll::Pending
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut task::Context) -> task::Poll<io::Result<()>> {
        task::Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _cx: &mut task::Context) -> task::Poll<io::Result<()>> {
        self.reactor.reset_write_waker();
        task::Poll::Ready(Ok(()))
    }
}

impl Drop for TcpStream {
    fn drop(&mut self) {
        trace!("TcpStream dropped");
        let _ = self.reactor.deregister(&self.sock);
    }
}
