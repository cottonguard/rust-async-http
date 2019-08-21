use log::*;
use mio::*;
use slab::Slab;
use std::cell::RefCell;
use std::io;
use std::task::Waker;

thread_local! {
    static REACTOR: RefCell<Reactor> = RefCell::new(Reactor::new().unwrap());
}

struct Reactor {
    poll: Poll,
    events: Events,
    nodes: Slab<Node>,
}

struct Node {
    readiness: Ready,
    read_waker: Waker,
    write_waker: Waker,
}

impl Reactor {
    fn new() -> io::Result<Reactor> {
        Ok(Reactor {
            poll: mio::Poll::new()?,
            events: mio::Events::with_capacity(1024),
            nodes: Slab::new(),
        })
    }

    fn register<E: ?Sized + Evented>(
        &mut self,
        handle: &E,
        read_waker: Waker,
        write_waker: Waker,
        interest: Ready,
    ) -> io::Result<ReactorHandle> {
        let key = self.nodes.insert(Node {
            readiness: Ready::empty(),
            read_waker,
            write_waker,
        });
        self.poll
            .register(handle, Token(key), interest, PollOpt::edge())?;
        Ok(ReactorHandle::new(key))
    }

    fn deregister<E: Evented>(&mut self, key: usize, handle: &E) -> io::Result<()> {
        self.poll.deregister(handle)?;
        self.nodes.remove(key);
        Ok(())
    }

    fn turn(&mut self, timeout: Option<std::time::Duration>) -> io::Result<usize> {
        trace!("begin turn");
        let n = self.poll.poll(&mut self.events, timeout)?;
        for event in &self.events {
            trace!("evented {:?}", &event);
            if let Some(node) = self.nodes.get_mut(event.token().0) {
                node.readiness |= event.readiness();
                if event.readiness().is_readable() {
                    node.read_waker.wake_by_ref();
                }
                if event.readiness().is_writable() {
                    node.write_waker.wake_by_ref();
                }
            }
        }
        Ok(n)
    }

    fn readiness(&self, key: usize) -> Option<Ready> {
        self.nodes.get(key).map(|node| node.readiness)
    }

    fn remove_readiness<R: Into<Ready>>(&mut self, key: usize, ready: R) {
        if let Some(node) = self.nodes.get_mut(key) {
            node.readiness.remove(ready);
        }
    }

    fn set_read_waker(&mut self, key: usize, waker: Waker) {
        if let Some(node) = self.nodes.get_mut(key) {
            node.read_waker = waker;
        }
    }

    fn set_write_waker(&mut self, key: usize, waker: Waker) {
        if let Some(node) = self.nodes.get_mut(key) {
            node.write_waker = waker;
        }
    }
}

pub fn register<E: ?Sized + Evented>(handle: &E, interest: Ready) -> io::Result<ReactorHandle> {
    REACTOR.with(|reactor| {
        reactor.borrow_mut().register(
            handle,
            futures::task::noop_waker(),
            futures::task::noop_waker(),
            interest,
        )
    })
}

pub fn turn(timeout: Option<std::time::Duration>) -> io::Result<usize> {
    REACTOR.with(|reactor| reactor.borrow_mut().turn(timeout))
}

#[derive(Debug)]
pub struct ReactorHandle {
    key: usize,
}

impl ReactorHandle {
    fn new(key: usize) -> ReactorHandle {
        ReactorHandle { key }
    }

    pub fn readiness(&self) -> Ready {
        REACTOR.with(|reactor| reactor.borrow().readiness(self.key).unwrap())
    }

    pub fn remove_readiness<R: Into<Ready>>(&self, ready: R) {
        REACTOR.with(|reactor| reactor.borrow_mut().remove_readiness(self.key, ready))
    }

    pub fn set_read_waker(&self, waker: Waker) {
        REACTOR.with(|reactor| reactor.borrow_mut().set_read_waker(self.key, waker))
    }

    pub fn reset_read_waker(&self) {
        REACTOR.with(|reactor| {
            reactor
                .borrow_mut()
                .set_read_waker(self.key, futures::task::noop_waker())
        })
    }

    pub fn set_write_waker(&self, waker: Waker) {
        REACTOR.with(|reactor| reactor.borrow_mut().set_write_waker(self.key, waker))
    }

    pub fn reset_write_waker(&self) {
        REACTOR.with(|reactor| {
            reactor
                .borrow_mut()
                .set_write_waker(self.key, futures::task::noop_waker())
        })
    }

    pub fn deregister<E: Evented>(&self, handle: &E) -> io::Result<()> {
        REACTOR.with(|reactor| reactor.borrow_mut().deregister(self.key, handle))
    }
}

impl Drop for ReactorHandle {
    fn drop(&mut self) {
        // deregister
    }
}
