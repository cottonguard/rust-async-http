use crate::reactor;
use futures::io::AsyncRead;
use lazy_static::*;
use log::*;
use mio::*;
use std::{
    collections::HashMap,
    fs,
    future::Future,
    io::{self, prelude::*},
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc, Mutex,
    },
    task::{self, Context},
    thread,
};

pub struct File {
    file: fs::File,
    registration: Registration,
    set_readiness: SetReadiness,
    reactor: reactor::ReactorHandle,
    read_handle: Option<ReadHandle<'static>>,
}

impl File {
    pub async fn open<P: AsRef<Path>>(path: P) -> io::Result<File> {
        let (registration, set_readiness) = Registration::new2();
        let reactor = reactor::register(&registration, Ready::readable())?;
        let handle = fs_queue().push_open(path, set_readiness.clone());
        let file = handle.await;
        file.map(|file| File {
            file,
            registration,
            set_readiness,
            reactor,
            read_handle: None,
        })
    }

    pub fn std(&self) -> &fs::File {
        &self.file
    }
}

impl AsyncRead for File {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> task::Poll<io::Result<usize>> {
        if self.read_handle.is_none() {
            let file_cloned = self.file.try_clone().unwrap(); // TODO: avoid cloning
            self.read_handle =
                Some(fs_queue().push_read(file_cloned, buf.len(), self.set_readiness.clone()));
        }
        let poll = Pin::new(self.read_handle.as_mut().unwrap())
            .poll(cx)
            .map(|res| {
                res.map(|src| {
                    buf[..src.len()].copy_from_slice(&src);
                    src.len()
                })
            });
        if poll.is_ready() {
            self.read_handle = None;
        }
        poll
    }
}

impl Evented for File {
    fn register(
        &self,
        poll: &Poll,
        token: Token,
        interest: mio::Ready,
        opts: mio::PollOpt,
    ) -> io::Result<()> {
        self.registration.register(poll, token, interest, opts)
    }

    fn reregister(
        &self,
        poll: &Poll,
        token: Token,
        interest: Ready,
        opts: PollOpt,
    ) -> io::Result<()> {
        self.registration.reregister(poll, token, interest, opts)
    }

    fn deregister(&self, poll: &Poll) -> io::Result<()> {
        poll.deregister(&self.registration)
    }
}

impl Drop for File {
    fn drop(&mut self) {
        let _ = self.reactor.deregister(&self.registration);
    }
}

lazy_static! {
    static ref FS_QUEUE: FsQueue = FsQueue::spawn();
}

fn fs_queue() -> &'static FsQueue {
    &FS_QUEUE
}

struct FsTask {
    token: usize,
    content: FsTaskContent,
    set_readiness: SetReadiness,
}

enum FsTaskContent {
    Open(PathBuf),
    Read(fs::File, usize),
}

struct FsResult {
    token: usize,
    content: FsResultContent,
}

enum FsResultContent {
    Open(io::Result<fs::File>),
    Read(io::Result<Vec<u8>>),
}

struct FsQueue {
    task_tx: mpsc::Sender<FsTask>,
    result_rx: mpsc::Receiver<FsResult>,
    result_map: Mutex<HashMap<usize, FsResult>>,
    next_token: AtomicUsize,
}

unsafe impl Sync for FsQueue {}

impl FsQueue {
    fn spawn() -> FsQueue {
        let (task_tx, task_rx) = mpsc::channel::<FsTask>();
        let (result_tx, result_rx) = mpsc::channel();
        let _handle = thread::spawn(move || {
            for task in task_rx {
                let (res, readiness) = match task.content {
                    FsTaskContent::Open(path) => (
                        FsResultContent::Open(fs::File::open(&path)),
                        Ready::readable(),
                    ),
                    FsTaskContent::Read(mut file, len) => (
                        FsResultContent::Read(Self::read(&mut file, len)),
                        Ready::readable(),
                    ),
                };
                let _ = task.set_readiness.set_readiness(readiness);
                if result_tx
                    .send(FsResult {
                        token: task.token,
                        content: res,
                    })
                    .is_err()
                {
                    break;
                }
            }
        });

        FsQueue {
            task_tx,
            result_rx,
            result_map: Mutex::new(HashMap::new()),
            next_token: AtomicUsize::new(1),
        }
    }

    fn read(file: &mut fs::File, max_len: usize) -> io::Result<Vec<u8>> {
        let len = max_len.min(file.metadata()?.len() as usize);
        let mut buf = vec![0; len];
        let res = file.read(&mut buf);
        res.map(|len| {
            buf.resize(len, 0);
            buf
        })
    }

    fn push_task(&self, content: FsTaskContent, set_readiness: SetReadiness) -> FsQueueHandle {
        let token = self.next_token.fetch_add(1, Ordering::SeqCst);
        self.task_tx
            .send(FsTask {
                content,
                token,
                set_readiness,
            })
            .unwrap();
        FsQueueHandle { token, que: &self }
    }

    fn push_open<P: AsRef<Path>>(&self, path: P, set_readiness: SetReadiness) -> OpenHandle {
        OpenHandle {
            inner: self.push_task(FsTaskContent::Open(path.as_ref().to_owned()), set_readiness),
        }
    }

    fn push_read(&self, file: fs::File, len: usize, set_readiness: SetReadiness) -> ReadHandle {
        ReadHandle {
            inner: self.push_task(FsTaskContent::Read(file, len), set_readiness),
        }
    }

    fn move_results(&self) {
        if let Ok(mut map) = self.result_map.lock() {
            for res in self.result_rx.try_iter() {
                map.insert(res.token, res);
            }
        }
    }

    fn result(&self, key: usize) -> Option<FsResult> {
        self.move_results(); // TODO: calls fewer
        if let Ok(mut map) = self.result_map.lock() {
            map.remove(&key)
        } else {
            None
        }
    }
}

struct FsQueueHandle<'a> {
    token: usize,
    que: &'a FsQueue,
}

impl<'a> Future for FsQueueHandle<'a> {
    type Output = FsResultContent;
    fn poll(self: Pin<&mut Self>, _cx: &mut Context) -> task::Poll<Self::Output> {
        if let Some(res) = self.que.result(self.token) {
            task::Poll::Ready(res.content)
        } else {
            task::Poll::Pending
        }
    }
}

struct OpenHandle<'a> {
    inner: FsQueueHandle<'a>,
}

impl<'a> Future for OpenHandle<'a> {
    type Output = io::Result<fs::File>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> task::Poll<Self::Output> {
        Pin::new(&mut self.inner).poll(cx).map(|res| {
            if let FsResultContent::Open(file) = res {
                file
            } else {
                panic!("result type is not open");
            }
        })
    }
}

struct ReadHandle<'a> {
    inner: FsQueueHandle<'a>,
}

impl<'a> Future for ReadHandle<'a> {
    type Output = io::Result<Vec<u8>>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> task::Poll<Self::Output> {
        Pin::new(&mut self.inner).poll(cx).map(|res| {
            if let FsResultContent::Read(buf) = res {
                buf
            } else {
                panic!("result type is not read");
            }
        })
    }
}
