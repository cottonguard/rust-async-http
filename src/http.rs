use crate::net::*;
use crate::reactor;
use crate::runner::{Runner, Spawner};
use futures::prelude::*;
use log::*;
use std::{collections::HashMap, future::Future, io, rc::Rc};

pub trait HttpApp {
    type Output: Future<Output = Response>;
    // TODO: &self to &mut self
    fn app(&self, req: Request) -> Self::Output;
}

impl<F: Fn(Request) -> T, T> HttpApp for F
where
    T: Future<Output = Response>,
{
    type Output = T;
    fn app(&self, req: Request) -> T {
        self(req)
    }
}

pub struct HttpServer<'a, T> {
    runner: Runner<'a>,
    inner: Rc<HttpServerInner<'a, T>>,
}

struct HttpServerInner<'a, T> {
    tcp: TcpListener,
    app: T,
    spawner: Spawner<'a>,
}

impl<'a, T: HttpApp + 'a> HttpServer<'a, T> {
    pub fn bind(addr: &std::net::SocketAddr, app: T) -> io::Result<Self> {
        let runner = Runner::new();
        Ok(HttpServer {
            inner: Rc::new(HttpServerInner {
                tcp: TcpListener::bind(addr)?,
                app,
                spawner: runner.spawner(),
            }),
            runner,
        })
    }

    pub fn run(mut self) -> io::Result<()> {
        self.inner.spawner.spawn(Rc::clone(&self.inner).accept());
        loop {
            reactor::turn(None)?;
            self.runner.run();
        }
    }
}

impl<'a, T: HttpApp + 'a> HttpServerInner<'a, T> {
    async fn accept(self: Rc<Self>) {
        loop {
            match self.tcp.accept().await {
                Ok((sock, addr)) => {
                    info!("accepted: {}", addr);
                    let cloned = Rc::clone(&self);
                    self.spawner.spawn(cloned.connection(sock));
                }
                Err(e) => {
                    warn!("{:?}", e);
                }
            }
        }
    }

    async fn connection(self: Rc<Self>, mut sock: TcpStream) {
        if let Err(e) = self.connection_inner(&mut sock).await {
            warn!("{:?}", e);
        }
    }

    async fn connection_inner(&self, sock: &mut TcpStream) -> io::Result<()> {
        let mut buf = vec![0u8; 1024];
        let len = sock.read(&mut buf).await?;
        trace!(
            "incoming message from {} ({} bytes):\n{}",
            sock.peer_addr().unwrap(),
            len,
            String::from_utf8_lossy(&buf)
        );
        let req = Self::parse_header(&buf[..len]);
        if let Some(req) = req {
            let res = self.app.app(req).await;
            dbg!(res.status_code);
            Self::write_response(sock, &res).await?;
        }
        Ok(())
    }

    fn parse_header(msg: &[u8]) -> Option<Request> {
        let mut req = Request::empty();
        let msg = String::from_utf8_lossy(msg);
        for (i, s) in msg.lines().enumerate() {
            if i == 0 {
                let tokens: Vec<_> = s.split(' ').collect();
                if tokens.len() != 3 {
                    return None;
                }
                req.method = tokens[0].to_owned();
                req.uri = tokens[1].to_owned();
                req.http_version = tokens[2].to_owned();
            } else {
                let kv: Vec<_> = s.splitn(2, ':').map(|s| s.trim()).collect();
                if kv.len() == 2 {
                    req.set_header(&kv[0].to_lowercase(), kv[1].to_owned());
                }
            }
        }
        dbg!(&req.headers);
        Some(req)
    }

    async fn write_response(sock: &mut TcpStream, res: &Response) -> io::Result<()> {
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

#[derive(Default)]
pub struct Request {
    method: String,
    uri: String,
    http_version: String,
    headers: HashMap<String, String>,
}

impl Request {
    pub fn empty() -> Request {
        Request::default()
    }

    pub fn http_version(&self) -> &str {
        &*self.http_version
    }

    pub fn method(&self) -> &str {
        &*self.method
    }

    pub fn uri(&self) -> &str {
        &*self.uri
    }

    // fixme
    pub fn url(&self) -> Result<url::Url, url::ParseError> {
        if let Some(host) = self.header("host") {
            Ok(url::Url::parse(&format!("http://{}", host))?.join(self.uri())?)
        } else {
            Err(url::ParseError::EmptyHost)
        }
    }

    pub fn header(&self, key: &str) -> Option<&str> {
        self.headers.get(key).map(|s| &**s)
    }

    pub fn set_header(&mut self, key: &str, value: String) -> Option<String> {
        if let Some(v) = self.headers.get_mut(key) {
            Some(std::mem::replace(v, value))
        } else {
            self.headers.insert(key.to_owned(), value)
        }
    }
}

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
    pub fn code(self) -> u32 {
        self as u32
    }

    pub fn description(self) -> &'static str {
        use StatusCode::*;
        match self {
            Ok => "OK",
        }
    }
}
