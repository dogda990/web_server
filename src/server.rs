use crate::cli::{Cli, PythonMode};
use crate::python_bridge::{AppResponse, AppState, RequestData};
use anyhow::{Context, Result};
use bytes::{Bytes, BytesMut};
use crossbeam_channel::{Receiver, Sender, TrySendError, bounded};
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::header::{CONTENT_LENGTH, HeaderName, HeaderValue, SERVER};
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode, Version};
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder as AutoBuilder;
use socket2::{Domain, Protocol, Socket, Type};
use std::convert::Infallible;
use std::env;
use std::net::{SocketAddr, ToSocketAddrs};
#[cfg(unix)]
use std::os::fd::{FromRawFd, RawFd};
use std::sync::Arc;
use std::thread;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

struct PythonJob {
    request: RequestData,
    response_tx: oneshot::Sender<Result<AppResponse, String>>,
}

struct PythonExecPool {
    tx: Sender<PythonJob>,
}

enum ExecuteError {
    Overloaded,
    Failed(String),
}

impl PythonExecPool {
    fn new(
        state: Arc<AppState>,
        threads: usize,
        queue_capacity: usize,
        bounded_queue: bool,
    ) -> Result<Self> {
        let threads = threads.max(1);
        let (tx, rx): (Sender<PythonJob>, Receiver<PythonJob>) = if bounded_queue {
            bounded(queue_capacity.max(1))
        } else {
            crossbeam_channel::unbounded()
        };

        for idx in 0..threads {
            let state = state.clone();
            let rx = rx.clone();
            thread::Builder::new()
                .name(format!("kubstu-pyexec-{}", idx + 1))
                .spawn(move || {
                    while let Ok(job) = rx.recv() {
                        let result = state.execute(job.request).map_err(|e| e.to_string());
                        let _ = job.response_tx.send(result);
                    }
                })
                .with_context(|| format!("Failed to spawn Python worker thread {}", idx + 1))?;
        }

        Ok(Self { tx })
    }

    async fn execute(
        &self,
        request: RequestData,
    ) -> std::result::Result<AppResponse, ExecuteError> {
        let (response_tx, response_rx) = oneshot::channel();
        self.tx
            .try_send(PythonJob {
                request,
                response_tx,
            })
            .map_err(|e| match e {
                TrySendError::Full(_) => ExecuteError::Overloaded,
                TrySendError::Disconnected(_) => {
                    ExecuteError::Failed("Python execution queue closed".into())
                }
            })?;

        match response_rx.await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(err)) => Err(ExecuteError::Failed(format!(
                "Application execution failed: {err}"
            ))),
            Err(err) => Err(ExecuteError::Failed(format!(
                "Python worker response channel closed: {err}"
            ))),
        }
    }
}

struct PythonExecutor {
    state: Arc<AppState>,
    pool: Option<PythonExecPool>,
    mode: PythonMode,
}

impl PythonExecutor {
    fn new(cli: &Cli, state: Arc<AppState>) -> Result<Self> {
        let pool = if cli.python_mode == PythonMode::Pool {
            Some(PythonExecPool::new(
                state.clone(),
                cli.python_threads,
                cli.python_queue_capacity,
                cli.bounded_python_queue,
            )?)
        } else {
            None
        };
        Ok(Self {
            state,
            pool,
            mode: cli.python_mode,
        })
    }

    async fn execute(
        &self,
        request: RequestData,
    ) -> std::result::Result<AppResponse, ExecuteError> {
        match self.mode {
            PythonMode::Spawn => {
                let state = self.state.clone();
                let (response_tx, response_rx) = oneshot::channel();
                thread::Builder::new()
                    .name("kubstu-request".into())
                    .spawn(move || {
                        let result = state.execute(request).map_err(|e| e.to_string());
                        let _ = response_tx.send(result);
                    })
                    .map_err(|err| {
                        ExecuteError::Failed(format!("Python request thread failed: {err}"))
                    })?;
                match response_rx.await {
                    Ok(Ok(response)) => Ok(response),
                    Ok(Err(err)) => Err(ExecuteError::Failed(format!(
                        "Application execution failed: {err}"
                    ))),
                    Err(err) => Err(ExecuteError::Failed(format!(
                        "Python worker response channel closed: {err}"
                    ))),
                }
            }
            PythonMode::Pool => {
                self.pool
                    .as_ref()
                    .expect("pool mode initializes a pool")
                    .execute(request)
                    .await
            }
            PythonMode::Inline => self
                .state
                .execute(request)
                .map_err(|e| ExecuteError::Failed(e.to_string())),
            PythonMode::Blocking => {
                let state = self.state.clone();
                tokio::task::spawn_blocking(move || state.execute(request))
                    .await
                    .map_err(|e| ExecuteError::Failed(format!("Python blocking task failed: {e}")))?
                    .map_err(|e| ExecuteError::Failed(e.to_string()))
            }
        }
    }
}

pub async fn serve(cli: Cli, state: Arc<AppState>, reuse_port: bool) -> Result<()> {
    let listener = match inherited_listener_from_env()? {
        Some(listener) => listener,
        None => bind_listener(cli.host.as_str(), cli.port, reuse_port)
            .with_context(|| format!("Failed to bind {}:{}", cli.host, cli.port))?,
    };
    let executor = Arc::new(PythonExecutor::new(&cli, state)?);
    let max_body_bytes = cli.max_body_bytes;
    let compact_body_buffer = cli.compact_body_buffer;

    println!(
        "Listening on http://{}:{} ({:?})",
        cli.host, cli.port, cli.interface
    );

    loop {
        let (stream, remote_addr) = listener.accept().await?;
        let local_addr = stream.local_addr()?;
        let executor = executor.clone();

        tokio::spawn(async move {
            let service = service_fn(move |req: Request<Incoming>| {
                let executor = executor.clone();
                async move {
                    let response = handle_request(
                        req,
                        remote_addr,
                        local_addr,
                        executor,
                        max_body_bytes,
                        compact_body_buffer,
                    )
                    .await;
                    Ok::<_, Infallible>(response)
                }
            });

            let io = TokioIo::new(stream);
            let builder = AutoBuilder::new(TokioExecutor::new());

            if let Err(err) = builder.serve_connection_with_upgrades(io, service).await {
                let msg = err.to_string();
                if !msg.contains("connection closed before message completed") {
                    eprintln!("Connection error: {err}");
                }
            }
        });
    }
}

fn inherited_listener_from_env() -> Result<Option<TcpListener>> {
    #[cfg(unix)]
    {
        if let Ok(fd_str) = env::var("KUBSTU_LISTENER_FD") {
            let fd = fd_str
                .parse::<RawFd>()
                .with_context(|| format!("Invalid KUBSTU_LISTENER_FD value: {fd_str}"))?;
            let std_listener = unsafe { std::net::TcpListener::from_raw_fd(fd) };
            std_listener
                .set_nonblocking(true)
                .with_context(|| "Failed to set inherited listener to nonblocking mode")?;
            let listener = TcpListener::from_std(std_listener)
                .with_context(|| "Failed to convert inherited listener to tokio")?;
            return Ok(Some(listener));
        }
    }

    Ok(None)
}

fn bind_listener(host: &str, port: u16, reuse_port: bool) -> Result<TcpListener> {
    let std_listener = bind_std_listener(host, port, reuse_port)?;
    TcpListener::from_std(std_listener).with_context(|| "Failed to convert listener to tokio")
}

pub fn bind_std_listener(host: &str, port: u16, reuse_port: bool) -> Result<std::net::TcpListener> {
    let addr = resolve_bind_addr(host, port)?;
    let domain = if addr.is_ipv4() {
        Domain::IPV4
    } else {
        Domain::IPV6
    };

    let socket = Socket::new(domain, Type::STREAM, Some(Protocol::TCP))
        .with_context(|| "Failed to create TCP socket")?;
    socket
        .set_reuse_address(true)
        .with_context(|| "Failed to enable SO_REUSEADDR")?;
    #[cfg(unix)]
    if reuse_port {
        socket
            .set_reuse_port(true)
            .with_context(|| "Failed to enable SO_REUSEPORT")?;
    }

    socket
        .bind(&addr.into())
        .with_context(|| format!("Bind failed for {addr}"))?;
    socket
        .listen(2048)
        .with_context(|| "listen(2) failed for TCP socket")?;
    socket
        .set_nonblocking(true)
        .with_context(|| "Failed to set nonblocking mode")?;

    let std_listener: std::net::TcpListener = socket.into();
    Ok(std_listener)
}

fn resolve_bind_addr(host: &str, port: u16) -> Result<SocketAddr> {
    let mut addrs = (host, port)
        .to_socket_addrs()
        .with_context(|| format!("Unable to resolve {host}:{port}"))?;
    addrs
        .next()
        .ok_or_else(|| anyhow::anyhow!("No resolved socket addresses for {host}:{port}"))
}

async fn handle_request(
    req: Request<Incoming>,
    remote_addr: SocketAddr,
    local_addr: SocketAddr,
    executor: Arc<PythonExecutor>,
    max_body_bytes: usize,
    compact_body_buffer: bool,
) -> Response<Full<Bytes>> {
    let is_head = req.method() == Method::HEAD;

    match parse_request(
        req,
        remote_addr,
        local_addr,
        max_body_bytes,
        compact_body_buffer,
    )
    .await
    {
        Ok(request_data) => match executor.execute(request_data).await {
            Ok(response) => build_response(response, is_head),
            Err(ExecuteError::Overloaded) => error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "Python execution queue is full",
            ),
            Err(ExecuteError::Failed(err)) => internal_error_response(&err),
        },
        Err(ParseError::TooLarge) => {
            error_response(StatusCode::PAYLOAD_TOO_LARGE, "Request body too large")
        }
        Err(ParseError::Invalid(err)) => {
            error_response(StatusCode::BAD_REQUEST, &format!("Invalid request: {err}"))
        }
    }
}

enum ParseError {
    TooLarge,
    Invalid(String),
}

async fn parse_request(
    req: Request<Incoming>,
    remote_addr: SocketAddr,
    local_addr: SocketAddr,
    max_body_bytes: usize,
    compact_body_buffer: bool,
) -> std::result::Result<RequestData, ParseError> {
    let (parts, mut body) = req.into_parts();
    let content_length = parts
        .headers
        .get(CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok());
    if content_length.is_some_and(|n| n > max_body_bytes) {
        return Err(ParseError::TooLarge);
    }
    let mut chunks = Vec::new();
    let mut body_len = 0usize;
    let mut compact_body =
        compact_body_buffer.then(|| BytesMut::with_capacity(content_length.unwrap_or(0)));
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|e| ParseError::Invalid(e.to_string()))?;
        if let Ok(data) = frame.into_data() {
            body_len = body_len
                .checked_add(data.len())
                .ok_or(ParseError::TooLarge)?;
            if body_len > max_body_bytes {
                return Err(ParseError::TooLarge);
            }
            if let Some(buffer) = &mut compact_body {
                buffer.extend_from_slice(&data);
            } else {
                chunks.push(data);
            }
        }
    }
    let body = if let Some(buffer) = compact_body {
        Bytes::from(buffer.freeze())
    } else {
        let mut joined = Vec::with_capacity(body_len);
        for chunk in chunks {
            joined.extend_from_slice(&chunk);
        }
        Bytes::from(joined)
    };

    let mut headers = Vec::with_capacity(parts.headers.len());
    for (name, value) in &parts.headers {
        headers.push((
            Bytes::copy_from_slice(name.as_str().as_bytes()),
            Bytes::copy_from_slice(value.as_bytes()),
        ));
    }

    Ok(RequestData {
        method: parts.method.as_str().to_string(),
        http_version: http_version(parts.version).to_string(),
        path: parts.uri.path().to_string(),
        query_string: parts.uri.query().unwrap_or("").to_string(),
        headers,
        body,
        client_host: remote_addr.ip().to_string(),
        client_port: remote_addr.port(),
        server_host: local_addr.ip().to_string(),
        server_port: local_addr.port(),
    })
}

fn http_version(version: Version) -> &'static str {
    match version {
        Version::HTTP_10 => "1.0",
        Version::HTTP_11 => "1.1",
        Version::HTTP_2 => "2",
        Version::HTTP_3 => "3",
        _ => "1.1",
    }
}

fn build_response(response: AppResponse, is_head: bool) -> Response<Full<Bytes>> {
    let mut builder = Response::builder().status(response.status);

    if let Some(headers) = builder.headers_mut() {
        let mut has_server = false;
        for (name, value) in response.headers {
            if name.as_ref().eq_ignore_ascii_case(b"server") {
                has_server = true;
            }

            if let (Ok(name), Ok(value)) = (
                HeaderName::from_bytes(name.as_ref()),
                HeaderValue::from_bytes(value.as_ref()),
            ) {
                headers.append(name, value);
            }
        }

        if !has_server {
            headers.insert(SERVER, HeaderValue::from_static("kubstu-web-server"));
        }
    }

    let body = if is_head { Bytes::new() } else { response.body };
    builder
        .body(Full::new(body))
        .unwrap_or_else(|_| internal_error_response("Response build failure"))
}

fn internal_error_response(message: &str) -> Response<Full<Bytes>> {
    error_response(StatusCode::INTERNAL_SERVER_ERROR, message)
}

fn error_response(status: StatusCode, message: &str) -> Response<Full<Bytes>> {
    let body = Full::new(Bytes::copy_from_slice(message.as_bytes()));
    let mut response = Response::new(body);
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(SERVER, HeaderValue::from_static("kubstu-web-server"));
    response
}
