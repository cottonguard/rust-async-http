use crate::net::*;
use crate::reactor;
use futures::{executor, prelude::*, task::*};
use log::*;
use std::{cell::RefCell, collections::HashMap, io, rc::Rc};

type ResponceFuture = future::LocalBoxFuture<'static, Response>;

pub trait HttpApp {
    fn app(&mut self, req: Request) -> ResponceFuture;
}

impl<F: Fn(Request) -> T, T> HttpApp for F
where
    T: std::future::Future<Output = Response> + 'static,
{
    fn app(&mut self, req: Request) -> ResponceFuture {
        Box::pin(self(req))
    }
}

pub struct HttpServer {
    // tcp: WrappedTcpListener,
    addr: std::net::SocketAddr,
}

impl HttpServer {
    pub fn bind(addr: &std::net::SocketAddr) -> io::Result<HttpServer> {
        Ok(HttpServer {
            addr: addr.clone(), // tcp: Rc::new(RefCell::new(TcpListener::bind(addr)?)),
        })
    }

    pub fn run<T: HttpApp + 'static>(self, app: T) -> io::Result<()> {
        let runner = HttpServerRunner::new(TcpListener::bind(&self.addr)?, app);
        runner.run()?;
        Ok(())
    }
}

struct HttpServerRunner<T> {
    tcp: RefCell<TcpListener>,
    app: RefCell<T>,
}

impl<T: HttpApp + 'static> HttpServerRunner<T> {
    fn new(tcp: TcpListener, app: T) -> Rc<Self> {
        Rc::new(HttpServerRunner {
            tcp: RefCell::new(tcp),
            app: RefCell::new(app),
        })
    }

    fn run(self: Rc<Self>) -> io::Result<()> {
        let mut pool = executor::LocalPool::new();
        pool.spawner()
            .spawn_local(self.accept_loop(pool.spawner()))
            .expect("executor error");
        loop {
            reactor::turn(None)?;
            pool.run_until_stalled();
        }
    }

    async fn accept_loop(self: Rc<Self>, mut spawner: executor::LocalSpawner) {
        loop {
            let this = Rc::clone(&self);
            let mut tcp = this.tcp.borrow_mut();
            match tcp.accept().await {
                Ok((sock, addr)) => {
                    info!("connected: {}", addr);
                    let this = Rc::clone(&self);
                    spawner
                        .spawn_local(this.connection(sock))
                        .expect("executor error");
                }
                Err(e) => {
                    warn!("{:?}", e);
                }
            }
        }
    }

    async fn connection(self: Rc<Self>, mut sock: TcpStream) {
        trace!("connection()");
        let _ = self.connection_inner(&mut sock).await;
    }

    async fn connection_inner(self: Rc<Self>, sock: &mut TcpStream) -> io::Result<()> {
        let mut buf = vec![0u8; 1024];
        let len = sock.read(&mut buf).await?;
        trace!(
            "incoming message from {} ({} bytes):\n{}",
            sock.peer_addr().unwrap(),
            len,
            String::from_utf8_lossy(&buf)
        );
        let req = Self::parse_header(&buf[..len]).await;

        let mut app = self.app.borrow_mut();
        let res = app.app(req).await;
        dbg!(res.status_code);
        Self::write_response(sock, &res).await?;
        Ok(())
    }

    async fn parse_header(msg: &[u8]) -> Request {
        Request {}
    }

    async fn write_response(sock: &mut TcpStream, res: &Response) -> io::Result<()> {
        res.set_header("Content-Length", format!("{}", res.body_len()));
        let mut w = futures::io::BufWriter::new(sock);
        let mut lines = vec![format!(
            "HTTP/1.1 {} {}",
            res.status_code().code(),
            res.status_code().description()
        )];
        lines.extend(res.headers().iter().map(|(k, v)| format!("{}: {}", k, v)));
        lines.push("".to_owned());
        lines.push("".to_owned());
        let header = lines.join("\r\n");
        w.write_all(header.as_bytes()).await?;
        w.write_all(res.body()).await?;
        w.flush().await?;
        Ok(())
    }
}

pub struct Request {}

pub struct Response {
    status_code: StatusCode,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

impl Response {
    pub fn with_status_code(status_code: StatusCode) -> Response {
        Response {
            status_code,
            headers: HashMap::new(),
            body: Vec::new(),
        }
    }

    pub fn ok() -> Response {
        Self::with_status_code(StatusCode::Ok)
    }

    pub fn status_code(&self) -> StatusCode {
        self.status_code
    }

    pub fn set_header(&mut self, key: &str, value: String) -> Option<String> {
        if let Some(v) = self.headers.get_mut(key) {
            Some(std::mem::replace(v, value))
        } else {
            self.headers.insert(key.to_owned(), value)
        }
    }

    pub fn headers(&self) -> &HashMap<String, String> {
        &self.headers
    }

    pub fn body(&self) -> &[u8] {
        &self.body
    }

    pub fn body_len(&self) -> usize {
        self.body().len()
    }
}

impl Extend<u8> for Response {
    fn extend<T: IntoIterator<Item = u8>>(&mut self, iter: T) {
        self.body.extend(iter);
    }
}

impl<'a> Extend<&'a u8> for Response {
    fn extend<T: IntoIterator<Item = &'a u8>>(&mut self, iter: T) {
        self.body.extend(iter.into_iter().copied());
    }
}

#[derive(Clone, Copy, Debug)]
pub enum StatusCode {
    Ok = 200,
}

impl StatusCode {
    pub fn code(&self) -> u32 {
        *self as u32
    }

    pub fn description(&self) -> &str {
        use StatusCode::*;
        match *self {
            Ok => "OK",
        }
    }
}
