use super::*;
use crate::model::MembershipTarget;
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tiny_http::{Header, Response, Server};

mod community_icon_lookups;
mod home_listings;
mod listing_request_urls;
mod media_request_headers;
mod membership;
mod post_listings;
mod private_community_denials;
mod rate_pacing;
mod reddit_health;
mod relay;
mod request_lifecycle;
mod saved_account_verification;
mod scoped_cookie_policy;
mod subscription_listings;
mod threads;

pub(crate) fn native_post(id: &str, subreddit: &str) -> String {
    format!(
        "<shreddit-post id='t3_{id}' post-title='{id}' subreddit-name='{subreddit}' author='reader' permalink='/r/{subreddit}/comments/{id}/title/' post-type='text'></shreddit-post>"
    )
}

pub(crate) fn saved_export(id: &str, name: &str) -> String {
    format!(
        "# redtab-connection {}\n.reddit.com\tTRUE\t/\tTRUE\t0\treddit_session\tfixture-session\n",
        json!({"account":{"id":id,"name":name},"source":{"Browser":{"browser":"firefox","profile":"work","label":"Firefox · Work"}}})
    )
}

pub(crate) struct Request {
    pub(crate) url: String,
    pub(crate) method: String,
    pub(crate) body: String,
    pub(crate) headers: HashMap<String, String>,
}

pub(crate) struct Reply {
    pub(crate) status: u16,
    pub(crate) body: String,
    pub(crate) headers: Vec<(String, String)>,
}

impl Reply {
    pub(crate) fn json(body: Value) -> Self {
        Self {
            status: 200,
            body: body.to_string(),
            headers: vec![],
        }
    }
    pub(crate) fn status(status: u16) -> Self {
        Self {
            status,
            body: "body must never appear in errors".into(),
            headers: vec![],
        }
    }
}

pub(crate) struct Fixture {
    pub(crate) base: String,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

/// The macOS application firewall can hold a freshly built test binary's first
/// connections to its own listener for longer than the client's connect timeout, so
/// each test server is connected to once, and waited for, before a test uses it.
const FIREWALL_HOLD: Duration = Duration::from_secs(30);

pub(crate) fn local_http_server() -> Server {
    let server = Server::http("127.0.0.1:0").unwrap();
    let address = server.server_addr().to_ip().unwrap();
    let _ = std::net::TcpStream::connect_timeout(&address, FIREWALL_HOLD);
    server
}

/// Like [`local_http_server`]; the warm-up connection is accepted and dropped here, so
/// callers that count accepted connections see only their own.
pub(crate) fn local_listener() -> std::net::TcpListener {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    if std::net::TcpStream::connect_timeout(&address, FIREWALL_HOLD).is_ok() {
        let _ = listener.accept();
    }
    listener
}

impl Fixture {
    pub(crate) fn new(handler: impl Fn(Request) -> Reply + Send + 'static) -> Self {
        let server = local_http_server();
        let base = format!("http://{}/", server.server_addr());
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let thread = std::thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                if let Some(mut request) = server.recv_timeout(Duration::from_millis(20)).unwrap() {
                    // The host's portkeeper probes new listeners; redtab never sends HEAD /.
                    if request.method() == &tiny_http::Method::Head && request.url() == "/" {
                        let _ = request.respond(Response::empty(200));
                        continue;
                    }
                    let mut body = String::new();
                    request
                        .as_reader()
                        .take(1024 * 1024)
                        .read_to_string(&mut body)
                        .unwrap();
                    let input = Request {
                        url: request.url().to_owned(),
                        method: request.method().as_str().to_owned(),
                        body,
                        headers: request
                            .headers()
                            .iter()
                            .map(|h| {
                                (
                                    h.field.to_string().to_ascii_lowercase(),
                                    h.value.to_string(),
                                )
                            })
                            .collect(),
                    };
                    let reply = handler(input);
                    let mut response =
                        Response::from_string(reply.body).with_status_code(reply.status);
                    for (name, value) in reply.headers {
                        response.add_header(Header::from_bytes(name, value).unwrap());
                    }
                    let _ = request.respond(response);
                }
            }
        });
        Self {
            base,
            stop,
            thread: Some(thread),
        }
    }
    pub(crate) fn reddit(&self) -> Reddit {
        Reddit::test_at(&self.base, false)
    }
    pub(crate) fn saved_reddit(&self, text: &str) -> Reddit {
        Reddit::with_transport(
            Transport::new().unwrap(),
            false,
            &self.base,
            session::Session::stored(text, session::now()).unwrap(),
        )
        .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let result = self.thread.take().unwrap().join();
        if !std::thread::panicking() {
            result.expect("fixture server");
        }
    }
}

fn empty_listing() -> Value {
    json!({"kind":"Listing","data":{"children":[],"after":null}})
}

fn home_post(id: &str, subreddit: &str) -> Value {
    json!({"kind":"t3","data":{"id":id,"title":id,"subreddit":subreddit,"permalink":format!("/r/{subreddit}/comments/{id}/title/"),"author":"reader"}})
}

/// Serves `body` from `offset` (the client's range), cutting the first response at a
/// third; a resume gets the rest as 206, or the whole body as 200 when `refuse`.
fn truncating_upstream(
    body: Vec<u8>,
    refuse: bool,
) -> (String, std::thread::JoinHandle<Vec<Option<usize>>>) {
    use std::io::{BufRead, Write};
    let listener = local_listener();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let handle = std::thread::spawn(move || {
        let mut ranges = Vec::new();
        for attempt in 0..2 {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
            let mut range = None;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("range: bytes=") {
                    range = value
                        .trim()
                        .split('-')
                        .next()
                        .unwrap()
                        .parse::<usize>()
                        .ok();
                }
            }
            ranges.push(range);
            let total = body.len();
            let from = range.unwrap_or(0);
            if attempt == 0 {
                let cut = from + (total - from) / 3;
                if range.is_some() {
                    write!(
                        stream,
                        "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {from}-{}/{total}\r\nContent-Length: {}\r\n\r\n",
                        total - 1,
                        total - from
                    )
                    .unwrap();
                } else {
                    write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {total}\r\n\r\n").unwrap();
                }
                stream.write_all(&body[from..cut]).unwrap();
                stream.flush().unwrap();
                // A close in the same packet fails the relay's first read, not a later one.
                std::thread::sleep(Duration::from_millis(200));
            } else if refuse {
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {total}\r\n\r\n").unwrap();
                stream.write_all(&body).unwrap();
            } else {
                write!(
                    stream,
                    "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {from}-{}/{total}\r\nContent-Length: {}\r\n\r\n",
                    total - 1,
                    total - from
                )
                .unwrap();
                stream.write_all(&body[from..]).unwrap();
            }
            stream.shutdown(std::net::Shutdown::Both).unwrap();
        }
        ranges
    });
    (base, handle)
}

/// The relayed body for a GET with an optional `Range` header.
fn relayed_body(reddit: &Reddit, url: &str, range: Option<&str>) -> Vec<u8> {
    use std::io::{Read, Write};
    let local = url::Url::parse(&reddit.media_url(url).unwrap()).unwrap();
    let mut client =
        std::net::TcpStream::connect(format!("127.0.0.1:{}", local.port().unwrap())).unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let range = range.map_or(String::new(), |range| format!("Range: {range}\r\n"));
    write!(
        client,
        "GET {} HTTP/1.1\r\nHost: 127.0.0.1\r\n{range}\r\n",
        local.path()
    )
    .unwrap();
    let mut response = Vec::new();
    let _ = client.read_to_end(&mut response);
    let start = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap()
        + 4;
    response[start..].to_vec()
}
