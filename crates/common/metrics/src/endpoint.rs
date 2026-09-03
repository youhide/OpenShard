//! The socket a shard answers questions about itself on.
//!
//! `GET /metrics` for a scraper, `GET /health` for a person or a launcher, and
//! `GET /` for whoever arrived without knowing either.
//!
//! # Why the HTTP is written here rather than pulled in
//!
//! What this serves is three routes, no request body, no keep-alive, no
//! compression, no TLS and no routing table — a response is a status line, three
//! headers and a string. An HTTP framework brings a runtime-integrated server, a
//! router, an extractor layer and several dozen crates of dependency, all of
//! which would have to pass the licence gate
//! `plans/server/operations/PLAN.md` still owes the tree, to write about forty
//! lines of text. The rules this owes in exchange are small and stated in
//! [`write_answer`]: a status line, a `content-length` that matches the body,
//! and a connection that closes when it says it will.
//!
//! # One request at a time, and that is deliberate
//!
//! There is no task per connection here. `docs/server/README.md` § what is open
//! ranks two unbounded queues as the shard's worst standing defects, and a
//! diagnostic port that spawns an unbounded number of tasks would be a third —
//! reachable, by construction, by anything that can open a socket. A scrape
//! comes every few seconds from one or two collectors, so serialising them costs
//! nothing real, and what it buys is that the whole endpoint's cost is one
//! connection's cost.
//!
//! What that leaves is a client that connects and says nothing, holding the port
//! against the next scrape. [`REQUEST_DEADLINE`] is the bound on that, and it is
//! the reason there is one.
//!
//! # This port has no authentication
//!
//! Nothing here asks who is calling, so what it publishes — a shard's tick rate,
//! its connection count, how far behind its disk is — is public to whoever can
//! reach the socket. It is meant for a loopback or a private interface. The
//! shard warns at boot when it is bound anywhere else; the REST admin API of
//! `plans/server/operations/PLAN.md`, which will carry authority rather than
//! numbers, is where authentication belongs and is a separate build.

use std::future::Future;
use std::net::SocketAddr;
use std::time::Duration;

use tokio::io::{
    AsyncReadExt,
    AsyncWriteExt,
};
use tokio::net::{
    TcpListener,
    TcpStream,
};
use tracing::{
    debug,
    info,
};

use crate::shard::ShardMetrics;
use crate::{
    exposition,
    health,
};

/// How long a client has to send its request line and headers.
///
/// A bound on this endpoint's own teardown rather than anything an operator
/// tunes, so a constant and not a setting — the same argument the gateway's
/// `DRAIN_ON_STOP` makes. Five seconds is far past any real client on any real
/// network and short enough that one that says nothing cannot hold the port
/// across two scrapes.
const REQUEST_DEADLINE: Duration = Duration::from_secs(5);

/// The most request head this will read before giving up on it.
///
/// A `GET` with no body has a request line and whatever headers the client felt
/// like sending; 8 KiB is what a browser sends with every cookie it owns, and an
/// order of magnitude past what a scraper sends. Past it, the client is not
/// making a request this can answer, and the point of the limit is that reading
/// it costs a bounded amount of memory rather than as much as the client cares
/// to send.
const HEAD_LIMIT: usize = 8 * 1024;

/// Where the request head ends and, in a request with one, the body begins.
const END_OF_HEAD: &[u8] = b"\r\n\r\n";

/// The socket a shard answers questions about itself on.
#[derive(Debug)]
pub struct MetricsEndpoint {
    listener: TcpListener,
    metrics:  ShardMetrics,
    /// How long one client has to send its request. A field rather than the
    /// constant read at the point of use, so that the test which proves the
    /// bound exists can prove it in milliseconds instead of waiting out the
    /// production value.
    deadline: Duration,
}

impl MetricsEndpoint {
    /// Bind, and read from `metrics` for as long as this serves.
    ///
    /// Bound before it is spawned, deliberately: a port already in use is a
    /// mistake in the config, and an operator should hear about it at boot
    /// rather than as a line in a log from a task that quietly gave up.
    pub async fn bind(address: SocketAddr, metrics: ShardMetrics) -> std::io::Result<Self> {
        let listener = TcpListener::bind(address).await?;
        Ok(Self {
            listener,
            metrics,
            deadline: REQUEST_DEADLINE,
        })
    }

    /// The same endpoint with a shorter patience, for the test that proves the
    /// patience runs out.
    #[cfg(test)]
    fn waiting(mut self, deadline: Duration) -> Self {
        self.deadline = deadline;
        self
    }

    /// The address actually bound, which matters when port 0 was requested.
    pub fn local_address(&self) -> std::io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// Answer until `stop` resolves.
    ///
    /// # What `stop` should be, and what it should not
    ///
    /// Not the shard's own stop signal. A shard that has been asked to stop is
    /// the shard an operator most wants to ask about — the last save can take as
    /// long as the store takes — and an endpoint that closed on the same word
    /// would answer "connection refused" for exactly that span. So the caller
    /// keeps this alive until the shard has finished going, and what this
    /// answers meanwhile is that it is no longer serving; see
    /// [`ShardMetrics::stopping`].
    ///
    /// Returns an error only if accepting itself fails, which means the listener
    /// is gone.
    pub async fn serve<Stop>(self, stop: Stop) -> std::io::Result<()>
    where
        Stop: Future<Output = ()>,
    {
        let Self {
            listener,
            metrics,
            deadline,
        } = self;
        info!(address = ?listener.local_addr()?, "metrics and health endpoint listening");
        tokio::pin!(stop);
        loop {
            let accepted = tokio::select! {
                // Biased towards the stop, as the gateway's accept loop is: a
                // connection arriving in the same moment is one nobody needs
                // answered.
                biased;

                () = &mut stop => {
                    info!("metrics endpoint stopping");
                    return Ok(());
                }

                accepted = listener.accept() => accepted?,
            };
            let (stream, peer) = accepted;
            // Awaited rather than spawned — see this module's header — but still
            // inside the stop, so a wedged client cannot hold a shutdown for the
            // whole request deadline.
            tokio::select! {
                biased;

                () = &mut stop => {
                    info!("metrics endpoint stopping");
                    return Ok(());
                }

                () = serve_one(stream, peer, &metrics, deadline) => {}
            }
        }
    }
}

/// Read one request, answer it, and hang up.
async fn serve_one(mut stream: TcpStream, peer: SocketAddr, metrics: &ShardMetrics, deadline: Duration) {
    let answer = match tokio::time::timeout(deadline, read_head(&mut stream)).await {
        // A client that connected and said nothing. There is nothing to answer
        // and no status code for "you did not ask", so it gets the hang-up.
        Err(_elapsed) => {
            debug!(%peer, "no request within the deadline");
            return;
        }
        Ok(Err(error)) => {
            debug!(%peer, %error, "reading the request failed");
            return;
        }
        // The client hung up mid-request, or sent nothing and closed. Same.
        Ok(Ok(Head::Closed)) => return,
        Ok(Ok(Head::TooLarge)) => Answer::text(Status::HeadTooLarge, "the request head is too large\n"),
        Ok(Ok(Head::Read(head))) => route(metrics, &head),
    };
    if let Err(error) = write_answer(&mut stream, answer).await {
        debug!(%peer, %error, "writing the response failed");
    }
}

/// What reading a request head produced.
///
/// Three outcomes rather than an `Option`, because "the client hung up" and "the
/// client sent more than this will read" are different things to do, and neither
/// of them is an I/O error.
#[derive(Debug)]
enum Head {
    Read(Vec<u8>),
    TooLarge,
    Closed,
}

/// Read up to the blank line that ends a request head.
///
/// The body, if the client sent one, is left unread: nothing here has a route
/// that takes one, and the connection closes after the answer.
async fn read_head(stream: &mut TcpStream) -> std::io::Result<Head> {
    let mut head = Vec::new();
    let mut chunk = [0_u8; 1024];
    loop {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Ok(match head.is_empty() {
                true => Head::Closed,
                // A head that ended without its blank line is not a request
                // this can answer, and saying so is more useful than silence.
                false => Head::Read(head),
            });
        }
        head.extend_from_slice(&chunk[..read]);
        if head
            .windows(END_OF_HEAD.len())
            .any(|window| window == END_OF_HEAD)
        {
            return Ok(Head::Read(head));
        }
        if head.len() > HEAD_LIMIT {
            return Ok(Head::TooLarge);
        }
    }
}

/// What the client asked for: the two tokens of a request line that matter.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Request<'a> {
    method: &'a str,
    /// The path, with any query string already cut off.
    path:   &'a str,
}

impl<'a> Request<'a> {
    /// Read a request line, or nothing if it is not one.
    ///
    /// Deliberately forgiving about the version token and unforgiving about the
    /// two before it: a request whose target this cannot read is one whose
    /// answer would be a guess.
    fn parse(head: &'a str) -> Option<Self> {
        let line = head.lines().next()?;
        let mut tokens = line.split(' ');
        let method = tokens.next()?;
        let target = tokens.next()?;
        if method.is_empty() || target.is_empty() {
            return None;
        }
        // `/metrics?foo=1` is `/metrics`, and a fragment never reaches a server
        // but costs nothing to ignore. `split` always yields a first field, even
        // for the empty string, so there is nothing here to be absent.
        let path = target
            .split(['?', '#'])
            .next()
            .expect("splitting a string always yields at least one field");
        Some(Self { method, path })
    }
}

/// Turn one request head into the answer it gets.
///
/// Pure, and separate from the socket, so that every route above has a test that
/// does not need one.
fn route(metrics: &ShardMetrics, head: &[u8]) -> Answer {
    let Ok(head) = std::str::from_utf8(head) else {
        return Answer::text(Status::BadRequest, "the request head is not UTF-8\n");
    };
    let Some(request) = Request::parse(head) else {
        return Answer::text(Status::BadRequest, "that is not a request line\n");
    };
    // `HEAD` is not answered either, and that is a choice rather than an
    // oversight: answering it correctly means rendering the body to get its
    // length and then throwing it away, and nothing that reads this endpoint
    // sends one.
    if request.method != "GET" {
        return Answer::text(Status::MethodNotAllowed, "only GET is answered here\n");
    }
    match trimmed(request.path) {
        "/metrics" => {
            Answer {
                status:       Status::Ok,
                content_type: exposition::CONTENT_TYPE,
                body:         exposition::render(&metrics.read()),
            }
        }
        "/health" => {
            let reading = metrics.read();
            Answer {
                // The status code answers one question — should play come here —
                // and the body answers the rest. A shard that is merely slow
                // still gets a 200: it is serving, and how slow is too slow is a
                // judgement this crate deliberately does not make.
                status:       match reading.serving() {
                    true => Status::Ok,
                    false => Status::NotServing,
                },
                content_type: health::CONTENT_TYPE,
                body:         health::render(&reading),
            }
        }
        "" | "/" => {
            Answer::text(
                Status::Ok,
                "OpenShard\n\n  /metrics  Prometheus text exposition\n  /health   the shard's own \
                 account of itself, as JSON\n",
            )
        }
        _ => Answer::text(Status::NotFound, "no such path; try /metrics or /health\n"),
    }
}

/// A path without its trailing slash, so `/health/` and `/health` are one route.
///
/// The root is left alone: trimming it would turn `/` into the empty string, and
/// the empty string is not a path anybody typed.
fn trimmed(path: &str) -> &str {
    match path.len() > 1 {
        true => path.trim_end_matches('/'),
        false => path,
    }
}

/// The status codes this endpoint has a reason to send.
///
/// A closed set rather than a number, because each of these is a decision made
/// somewhere above with a reason written beside it, and a bare `503` at a call
/// site says none of it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Status {
    Ok,
    BadRequest,
    NotFound,
    MethodNotAllowed,
    HeadTooLarge,
    /// The shard has been asked to stop. It still answers; it is no longer
    /// taking play, and anything deciding where to send a player must not.
    NotServing,
}

impl Status {
    /// The code and reason phrase, as they appear in a status line.
    const fn line(self) -> &'static str {
        match self {
            Self::Ok => "200 OK",
            Self::BadRequest => "400 Bad Request",
            Self::NotFound => "404 Not Found",
            Self::MethodNotAllowed => "405 Method Not Allowed",
            Self::HeadTooLarge => "431 Request Header Fields Too Large",
            Self::NotServing => "503 Service Unavailable",
        }
    }
}

/// One response, before it is bytes.
#[derive(Clone, PartialEq, Eq, Debug)]
struct Answer {
    status:       Status,
    content_type: &'static str,
    body:         String,
}

impl Answer {
    /// An answer that is a sentence rather than a document.
    fn text(status: Status, body: &str) -> Self {
        Self {
            status,
            content_type: "text/plain; charset=utf-8",
            body: body.to_owned(),
        }
    }
}

/// Write one response and close the connection.
///
/// The three rules this owes for writing HTTP by hand: the status line names a
/// version the client understands, `content-length` is the body's length in
/// bytes and not its length in characters, and `connection: close` is honoured
/// by actually closing — a client that was told the connection ends and then
/// left waiting is worse than one that was told nothing.
///
/// `cache-control: no-store` because both bodies are a measurement of this
/// instant, and a proxy that served yesterday's health document would be a
/// monitoring system reporting a shard that no longer exists.
async fn write_answer(stream: &mut TcpStream, answer: Answer) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 {}\r\ncontent-type: {}\r\ncontent-length: {}\r\ncache-control: \
         no-store\r\nconnection: close\r\n\r\n",
        answer.status.line(),
        answer.content_type,
        answer.body.len(),
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(answer.body.as_bytes()).await?;
    stream.flush().await?;
    stream.shutdown().await
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::time::Duration;

    use tokio::io::{
        AsyncReadExt,
        AsyncWriteExt,
    };
    use tokio::net::TcpStream;

    use super::{
        Answer,
        MetricsEndpoint,
        Request,
        Status,
        route,
    };
    use crate::shard::{
        ShardMetrics,
        TickInterval,
    };

    const DECLARED: TickInterval = TickInterval(Duration::from_millis(25));

    fn ask(metrics: &ShardMetrics, line: &str) -> Answer {
        route(metrics, format!("{line}\r\nhost: localhost\r\n\r\n").as_bytes())
    }

    #[test]
    fn a_query_string_does_not_make_a_new_route() {
        // A scraper that appends anything at all must not fall through to 404.
        let request = Request::parse("GET /metrics?collect=all HTTP/1.1\r\n").expect("a request line");
        assert_eq!(request.method, "GET");
        assert_eq!(request.path, "/metrics");
    }

    #[test]
    fn the_three_routes_answer_and_everything_else_does_not() {
        let metrics = ShardMetrics::declaring(DECLARED);

        assert_eq!(ask(&metrics, "GET / HTTP/1.1").status, Status::Ok);
        assert_eq!(ask(&metrics, "GET /metrics HTTP/1.1").status, Status::Ok);
        assert_eq!(ask(&metrics, "GET /health HTTP/1.1").status, Status::Ok);
        // A trailing slash is the same route; anything else is not a route.
        assert_eq!(ask(&metrics, "GET /health/ HTTP/1.1").status, Status::Ok);
        assert_eq!(ask(&metrics, "GET /admin HTTP/1.1").status, Status::NotFound);
    }

    #[test]
    fn a_write_is_refused_rather_than_ignored() {
        // This endpoint publishes and never accepts. A `POST` that came back 404
        // would read as "wrong path" and invite a retry against another one.
        let metrics = ShardMetrics::declaring(DECLARED);
        assert_eq!(
            ask(&metrics, "POST /metrics HTTP/1.1").status,
            Status::MethodNotAllowed
        );
    }

    #[test]
    fn a_head_that_is_not_a_request_is_refused_and_not_guessed_at() {
        let metrics = ShardMetrics::declaring(DECLARED);
        assert_eq!(route(&metrics, b"\r\n\r\n").status, Status::BadRequest);
        assert_eq!(route(&metrics, &[0xff, 0xfe]).status, Status::BadRequest);
    }

    #[test]
    fn a_stopping_shard_answers_health_with_the_code_that_says_so() {
        // The one place a measurement becomes a status code, and it is a fact
        // rather than a threshold: the shard was asked to stop.
        let metrics = ShardMetrics::declaring(DECLARED);
        assert_eq!(ask(&metrics, "GET /health HTTP/1.1").status, Status::Ok);

        metrics.stopping();
        assert_eq!(ask(&metrics, "GET /health HTTP/1.1").status, Status::NotServing);
        assert_eq!(
            ask(&metrics, "GET /metrics HTTP/1.1").status,
            Status::Ok,
            "a scrape is still a scrape while the world is being saved"
        );
    }

    /// One real GET over one real socket, head and body.
    async fn get(address: SocketAddr, path: &str) -> String {
        let mut stream = TcpStream::connect(address)
            .await
            .expect("the endpoint is listening");
        stream
            .write_all(
                format!("GET {path} HTTP/1.1\r\nhost: localhost\r\nconnection: close\r\n\r\n").as_bytes(),
            )
            .await
            .expect("the request was sent");
        let mut response = String::new();
        stream
            .read_to_string(&mut response)
            .await
            .expect("the response was read to the hang-up");
        response
    }

    #[tokio::test]
    async fn a_real_client_gets_a_response_it_can_read_to_the_end() {
        // The whole point of writing the HTTP by hand is that these three rules
        // are now this module's to keep: a status line, a content-length that
        // matches the body, and a connection that closes when it says it will.
        // A client blocked forever on a body that never ends is what breaking
        // any of them looks like, and `read_to_string` returning is the
        // assertion that none of them is broken.
        let metrics = ShardMetrics::declaring(DECLARED);
        metrics.tick_ran();
        let endpoint = MetricsEndpoint::bind(SocketAddr::from(([127, 0, 0, 1], 0)), metrics)
            .await
            .expect("a loopback port");
        let address = endpoint.local_address().expect("the bound address");
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let serving = tokio::spawn(endpoint.serve(async {
            let _ = stopped.await;
        }));

        let response = get(address, "/metrics").await;
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
        assert!(response.contains("openshard_ticks_total 1"), "{response}");

        let (head, body) = response.split_once("\r\n\r\n").expect("a head and a body");
        assert!(
            head.contains(&format!("content-length: {}", body.len())),
            "the length promised is not the length sent: {response}"
        );

        // A second request on a second connection, because the endpoint serves
        // one at a time and this is what proves it goes back for the next.
        let health = get(address, "/health").await;
        assert!(health.starts_with("HTTP/1.1 200 OK\r\n"), "{health}");
        assert!(health.contains("\"serving\":true"), "{health}");

        let missing = get(address, "/nothing").await;
        assert!(missing.starts_with("HTTP/1.1 404 Not Found\r\n"), "{missing}");

        let _ = stop.send(());
        tokio::time::timeout(Duration::from_secs(5), serving)
            .await
            .expect("the endpoint stopped when asked")
            .expect("and it did not panic")
            .expect("and it did not fail to accept");
    }

    #[tokio::test]
    async fn a_client_that_says_nothing_does_not_hold_the_port_forever() {
        // The bound that pays for serving one request at a time. Without it, a
        // single open socket is a metrics endpoint nobody can scrape again —
        // which is what makes this the one test the deadline exists for.
        let endpoint = MetricsEndpoint::bind(
            SocketAddr::from(([127, 0, 0, 1], 0)),
            ShardMetrics::declaring(DECLARED),
        )
        .await
        .expect("a loopback port")
        .waiting(Duration::from_millis(50));
        let address = endpoint.local_address().expect("the bound address");
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let serving = tokio::spawn(endpoint.serve(async {
            let _ = stopped.await;
        }));

        // Connected and silent, deliberately: this is the shape of the problem,
        // and it is held open across the scrape below rather than dropped.
        let silent = TcpStream::connect(address)
            .await
            .expect("the endpoint is listening");

        let response = tokio::time::timeout(Duration::from_secs(5), get(address, "/health"))
            .await
            .expect("the silent client was hung up on and the next one served");
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
        drop(silent);

        let _ = stop.send(());
        tokio::time::timeout(Duration::from_secs(5), serving)
            .await
            .expect("the endpoint stopped when asked")
            .expect("and it did not panic")
            .expect("and it did not fail to accept");
    }
}
