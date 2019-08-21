#![feature(async_await)]
#![feature(async_closure)]
#![allow(unused)]

use futures::executor;
use futures::prelude::*;
use futures::task::*;
use log::*;
use net_test3::*;
use std::cell::RefCell;
use std::rc::Rc;

fn main() -> std::io::Result<()> {
    env_logger::init();
    let addr = "127.0.0.1:8989".parse().unwrap();
    let mut http = http::HttpServer::bind(&addr, static_router::static_router)?;
    /*
    let mut http = http::HttpServer::bind(&addr, async move |req: http::Request| {
        let mut res = http::Response::ok();
        res.extend(b"Hello world!\n");
        res.extend(format!("{}\n", req.url()).as_bytes());
        res.set_header("Content-Type", "text/plain".to_owned());
        res
    })?;
    */
    info!("http server listening on {}", &addr);
    http.run();
    Ok(())
}
