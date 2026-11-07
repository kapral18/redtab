use super::*;
use crate::model::MembershipTarget;
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

mod community_icon_lookups;
mod home_listings;
mod listing_request_urls;
mod media_hosts;
mod membership;
mod post_listings;
mod private_community_denials;
mod rate_pacing;
mod reddit_health;
mod relay;
mod request_lifecycle;
mod saved_account_verification;
mod scoped_cookie_policy;
pub(crate) mod served;
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
    host: String,
}

type Handler = Box<dyn Fn(Request) -> Reply + Send>;

enum Answers {
    Handler(std::sync::Mutex<Handler>, AtomicBool),
    Served(std::sync::Mutex<std::sync::mpsc::Sender<served::Request>>),
}

/// Fixture hosts by name; the transport answers their requests in memory.
static FIXTURES: std::sync::LazyLock<std::sync::Mutex<HashMap<String, Arc<Answers>>>> =
    std::sync::LazyLock::new(Default::default);
static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);

fn register(answers: Answers) -> String {
    let host = format!(
        "fixture-{}.test",
        NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
    );
    FIXTURES
        .lock()
        .unwrap()
        .insert(host.clone(), Arc::new(answers));
    host
}

/// The transport's answer in tests: a fixture host or live relay answers in memory and
/// any other request fails as if the connection did. Answers run on their own thread, as a
/// server would, so a test can hold one while the client times out or is cancelled.
pub(crate) fn answer(
    request: wreq::RequestBuilder,
    timeout: Option<Duration>,
) -> transport::Sending {
    let answering = request
        .build()
        .ok()
        .and_then(|request| answer_request(request));
    Box::pin(async move {
        let answer = answering.ok_or(())?;
        let (tx, rx) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            let _ = tx.send(answer());
        });
        let answer = async { rx.await.ok().flatten().ok_or(()) };
        match timeout {
            Some(timeout) => tokio::time::timeout(timeout, answer)
                .await
                .map_err(|_| ())?,
            None => answer.await,
        }
    })
}

type Answer = Box<dyn FnOnce() -> Option<wreq::Response> + Send>;

fn answer_request(request: wreq::Request) -> Option<Answer> {
    let uri = request.uri().clone();
    let url = uri.to_string();
    let header = |name: &str| {
        request
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
    };
    if let Some(relayed) = super::relay::answer_local(&url, header("range")) {
        return Some(Box::new(move || {
            let relayed = relayed();
            let response = served::Response {
                status: relayed.status,
                headers: relayed
                    .headers
                    .into_iter()
                    .map(|(name, value)| (name.to_owned(), value))
                    .collect(),
                body: relayed.body,
                len: relayed.len.map(|len| len as usize),
            };
            Some(response.into_http(uri))
        }));
    }
    let answers = FIXTURES.lock().unwrap().get(uri.host()?)?.clone();
    let path = uri
        .path_and_query()
        .map_or_else(|| "/".into(), |path| path.to_string());
    let headers: Vec<(String, String)> = request
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_ascii_lowercase(),
                value.to_str().unwrap_or_default().to_owned(),
            )
        })
        .collect();
    let method = request.method().as_str().to_owned();
    let body = request
        .body()
        .and_then(|body| body.as_bytes())
        .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
        .unwrap_or_default();
    Some(Box::new(move || match &*answers {
        Answers::Handler(handler, panicked) => {
            let input = Request {
                url: path,
                method,
                body,
                headers: headers.into_iter().collect(),
            };
            let reply = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                (handler.lock().unwrap_or_else(|e| e.into_inner()))(input)
            }))
            .unwrap_or_else(|_| {
                panicked.store(true, Ordering::Relaxed);
                Reply::status(500)
            });
            let mut response =
                served::Response::from_string(reply.body).with_status_code(reply.status);
            response.headers = reply.headers;
            Some(response.into_http(uri))
        }
        Answers::Served(requests) => {
            let (reply, replied) = std::sync::mpsc::channel();
            let request = served::Request {
                url: path,
                method,
                headers: headers
                    .into_iter()
                    .map(|(field, value)| served::Header { field, value })
                    .collect(),
                reply,
            };
            requests.lock().unwrap().send(request).ok()?;
            Some(replied.recv().ok()?.into_http(uri))
        }
    }))
}

/// A server the test answers by hand, request by request, in memory.
pub(crate) fn local_http_server() -> served::Server {
    let (tx, rx) = std::sync::mpsc::channel();
    let host = register(Answers::Served(std::sync::Mutex::new(tx)));
    served::Server {
        fixture: Fixture {
            base: format!("http://{host}/"),
            host,
        },
        requests: std::sync::Mutex::new(rx),
    }
}

impl Fixture {
    pub(crate) fn new(handler: impl Fn(Request) -> Reply + Send + 'static) -> Self {
        let host = register(Answers::Handler(
            std::sync::Mutex::new(Box::new(handler)),
            AtomicBool::new(false),
        ));
        Self {
            base: format!("http://{host}/"),
            host,
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
        let answers = FIXTURES.lock().unwrap().remove(&self.host);
        // Requests still being answered hold their own reference; a panic there counts too.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while answers
            .as_ref()
            .is_some_and(|a| matches!(**a, Answers::Handler(..)) && Arc::strong_count(a) > 1)
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(5));
        }
        if !std::thread::panicking()
            && let Some(Answers::Handler(_, panicked)) = answers.as_deref()
            && panicked.load(Ordering::Relaxed)
        {
            panic!("fixture handler panicked");
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
    use served::{Header, Response};
    let server = local_http_server();
    let base = format!("http://{}/", server.server_addr());
    let handle = std::thread::spawn(move || {
        let mut ranges = Vec::new();
        for attempt in 0..2 {
            let Ok(Some(request)) = server.recv_timeout(Duration::from_secs(5)) else {
                break;
            };
            let range = request
                .headers()
                .iter()
                .find(|header| header.field == "range")
                .and_then(|header| header.value.strip_prefix("bytes="))
                .and_then(|value| value.split('-').next()?.parse::<usize>().ok());
            ranges.push(range);
            let total = body.len();
            let from = range.unwrap_or(0);
            let partial = |from: usize, to: usize| {
                let headers = vec![Header::new(
                    "Content-Range",
                    &format!("bytes {from}-{}/{total}", total - 1),
                )];
                Response::new(
                    206,
                    headers,
                    std::io::Cursor::new(body[from..to].to_vec()),
                    Some(total - from),
                )
            };
            let whole = |to: usize| {
                Response::new(
                    200,
                    vec![],
                    std::io::Cursor::new(body[..to].to_vec()),
                    Some(total),
                )
            };
            let response = match (attempt, range.is_some()) {
                (0, true) => partial(from, from + (total - from) / 3),
                (0, false) => whole(total / 3),
                _ if refuse => whole(total),
                _ => partial(from, total),
            };
            request.respond(response).unwrap();
        }
        ranges
    });
    (base, handle)
}

pub(crate) struct Relayed {
    pub(crate) status: u16,
    pub(crate) headers: Vec<(&'static str, String)>,
    pub(crate) body: Vec<u8>,
}

/// What a player receives from the relay for `url`, a relay URL, as far as its body reads.
pub(crate) fn relayed(url: &str, range: Option<&str>) -> Relayed {
    use std::io::Read;
    let response =
        super::relay::answer_local(url, range.map(str::to_owned)).expect("a live relay URL")();
    let mut body = Vec::new();
    let _ = match response.len {
        Some(len) => response.body.take(len).read_to_end(&mut body),
        None => {
            let mut reader = response.body;
            reader.read_to_end(&mut body)
        }
    };
    Relayed {
        status: response.status,
        headers: response.headers,
        body,
    }
}

/// The relayed body for a GET with an optional `Range` header.
fn relayed_body(reddit: &Reddit, url: &str, range: Option<&str>) -> Vec<u8> {
    relayed(&reddit.media_url(url).unwrap(), range).body
}

/// The video player's playlist GET, answered in memory.
pub(crate) fn fetch_text(url: &str) -> Result<String, ureq::Error> {
    use std::io::Read;
    let transport = Transport::new().unwrap();
    let failed = || ureq::Error::Io(std::io::Error::other("connection failed"));
    let response = transport::wait(
        transport::send(transport.client.get(url), Some(Duration::from_secs(15))),
        None,
    )
    .map_err(|_| failed())?
    .map_err(|_| failed())?;
    if !response.status().is_success() {
        return Err(ureq::Error::StatusCode(response.status().as_u16()));
    }
    let mut text = String::new();
    transport::Reader::new(response, 1 << 20, None)
        .read_to_string(&mut text)
        .map_err(ureq::Error::Io)?;
    Ok(text)
}
