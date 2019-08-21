use crate::fs;
use crate::http::*;
use futures::io::*;
use std::path::Path;

pub async fn static_router(req: Request) -> Response {
    let path = req.uri();
    if let Ok(meta) = std::fs::metadata(path) {
        if meta.is_dir() {
            if let Ok(res) = dir_page(path) {
                res
            } else {
                Response::ok()
            }
        } else {
            let mut res = Response::ok();
            if let Ok(mut file) = fs::File::open(req.uri()).await {
                let mut buf = vec![0; file.std().metadata().unwrap().len() as usize];
                if file.read(&mut buf).await.is_ok() {
                    res.extend(&buf);
                }
            }
            res
        }
    } else {
        Response::ok()
    }
}

fn dir_page<P: AsRef<Path>>(path: P) -> std::io::Result<Response> {
    let mut res = Response::ok();
    let dir = std::fs::read_dir(&path)?;
    res.extend(
        format!(
            "<html><head><title>{0}</title></head><body><h1>{0}</h1><ul>",
            path.as_ref().to_string_lossy()
        )
        .bytes(),
    );
    for e in dir {
        let e = e?;
        res.extend(
            format!(
                "<li><a href=\"{}\">{}</a>",
                e.path().to_string_lossy(),
                e.file_name().to_string_lossy(),
            )
            .bytes(),
        );
    }
    res.extend(b"</ol></body></html>");
    Ok(res)
}
