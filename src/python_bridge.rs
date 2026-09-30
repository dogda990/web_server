use crate::cli::{Cli, Interface};
use anyhow::{Result, anyhow};
use bytes::Bytes;
use pyo3::exceptions::PyTypeError;
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyList};
use std::path::Path;

type RawHeaders = Vec<(Vec<u8>, Vec<u8>)>;
type RawAsgiResponse = (u16, RawHeaders, Vec<u8>);

const HELPERS_CODE: &str = r#"
import asyncio
import io
import sys
import threading
import urllib.parse


_WSGI_BASE = {
    "GATEWAY_INTERFACE": "CGI/1.1",
    "SCRIPT_NAME": "",
    "SERVER_SOFTWARE": "kubstu-web-server",
    "wsgi.version": (1, 0),
    "wsgi.url_scheme": "http",
    "wsgi.multithread": True,
    "wsgi.multiprocess": False,
    "wsgi.run_once": False,
    "wsgi.errors": sys.stderr,
}


def _rust_wsgi_handle(app, environ):
    captured = {
        "status": "500 Internal Server Error",
        "headers": [],
    }
    chunks = []

    def write(data):
        if not isinstance(data, bytes):
            raise TypeError("WSGI write() requires bytes")
        chunks.append(data)

    def start_response(status, headers, exc_info=None):
        if exc_info is not None and chunks:
            raise exc_info[1].with_traceback(exc_info[2])
        captured["status"] = status
        captured["headers"] = headers
        return write

    response = app(environ, start_response)
    try:
        for chunk in response:
            write(chunk)
    finally:
        close = getattr(response, "close", None)
        if close is not None:
            close()

    return captured["status"], captured["headers"], b"".join(chunks)


async def _rust_asgi_collect(app, scope, body):
    status = 500
    headers = []
    chunks = []
    response_started = False
    response_finished = False
    body_delivered = False
    response_done = asyncio.Event()

    async def receive():
        nonlocal body_delivered
        if not body_delivered:
            body_delivered = True
            return {"type": "http.request", "body": body, "more_body": False}
        await response_done.wait()
        return {"type": "http.disconnect"}

    async def send(message):
        nonlocal status, headers, response_started, response_finished
        msg_type = message.get("type")
        if msg_type == "http.response.start":
            if response_started:
                raise RuntimeError("ASGI response started twice")
            response_started = True
            status = int(message.get("status", 500))
            headers = message.get("headers", [])
        elif msg_type == "http.response.body":
            if not response_started or response_finished:
                raise RuntimeError("Invalid ASGI response body order")
            chunks.append(message.get("body", b"") or b"")
            if not message.get("more_body", False):
                response_finished = True
                response_done.set()

    await app(scope, receive, send)
    response_done.set()
    if not response_started or not response_finished:
        raise RuntimeError("ASGI application did not finish its HTTP response")
    return status, headers, b"".join(chunks)


_asgi_loop = None
_asgi_loop_thread = None
_asgi_loop_ready = threading.Event()
_asgi_loop_lock = threading.Lock()


def _ensure_asgi_loop():
    global _asgi_loop, _asgi_loop_thread
    if _asgi_loop is not None and _asgi_loop.is_running():
        return _asgi_loop

    with _asgi_loop_lock:
        if _asgi_loop is not None and _asgi_loop.is_running():
            return _asgi_loop

        _asgi_loop_ready.clear()
        _asgi_loop = asyncio.new_event_loop()

        def _runner():
            asyncio.set_event_loop(_asgi_loop)
            _asgi_loop_ready.set()
            _asgi_loop.run_forever()

        _asgi_loop_thread = threading.Thread(
            target=_runner,
            name="kubstu-asgi-loop",
            daemon=True,
        )
        _asgi_loop_thread.start()
        _asgi_loop_ready.wait()
        return _asgi_loop


def _http_server_protocol(http_version):
    if http_version == "1.0":
        return "HTTP/1.0"
    if http_version == "2":
        return "HTTP/2"
    if http_version == "3":
        return "HTTP/3"
    return "HTTP/1.1"


def _rust_wsgi_handle_raw(
    app,
    method,
    http_version,
    path,
    query_string,
    headers,
    body,
    client_host,
    client_port,
    server_host,
    server_port,
    wsgi_multiprocess,
):
    environ = _WSGI_BASE.copy()
    environ["REQUEST_METHOD"] = method
    if "%" in path:
        environ["PATH_INFO"] = urllib.parse.unquote_to_bytes(path).decode("latin-1", errors="replace")
    else:
        environ["PATH_INFO"] = path
    environ["QUERY_STRING"] = query_string
    environ["SERVER_NAME"] = server_host
    environ["SERVER_PORT"] = str(server_port)
    environ["REMOTE_ADDR"] = client_host
    environ["REMOTE_PORT"] = str(client_port)
    environ["SERVER_PROTOCOL"] = _http_server_protocol(http_version)
    environ["wsgi.multiprocess"] = wsgi_multiprocess
    environ["wsgi.input"] = io.BytesIO(body)

    has_host = False
    for name_b, value_b in headers:
        name = name_b.decode("latin-1")
        lname = name.lower()
        value = value_b.decode("latin-1")

        if lname == "content-type":
            environ["CONTENT_TYPE"] = value
            continue
        if lname == "content-length":
            environ["CONTENT_LENGTH"] = value
            continue

        env_key = "HTTP_" + lname.replace("-", "_").upper()
        if env_key in environ:
            sep = ";" if lname == "cookie" else ","
            environ[env_key] = environ[env_key] + sep + value
        else:
            environ[env_key] = value

        if lname == "host":
            has_host = True

    if not has_host:
        environ["HTTP_HOST"] = f"{server_host}:{server_port}"

    return _rust_wsgi_handle(app, environ)


def _rust_asgi_handle_raw(
    app,
    method,
    http_version,
    path,
    query_string,
    headers,
    body,
    client_host,
    client_port,
    server_host,
    server_port,
):
    if not any(name.lower() == b"host" for name, _ in headers):
        headers = [(b"host", f"{server_host}:{server_port}".encode("latin-1"))] + list(headers)

    if "%" in path:
        asgi_path = urllib.parse.unquote(path, encoding="utf-8", errors="replace")
    else:
        asgi_path = path

    scope = {
        "type": "http",
        "asgi": {"version": "3.0", "spec_version": "2.3"},
        "http_version": http_version,
        "method": method,
        "scheme": "http",
        "root_path": "",
        "path": asgi_path,
        "raw_path": path.encode("latin-1", errors="replace"),
        "query_string": query_string.encode("latin-1", errors="replace"),
        "headers": list(headers),
        "client": (client_host, client_port),
        "server": (server_host, server_port),
        "state": {},
    }

    loop = _ensure_asgi_loop()
    fut = asyncio.run_coroutine_threadsafe(_rust_asgi_collect(app, scope, body), loop)
    return fut.result()
"#;

#[derive(Debug, Clone)]
pub struct RequestData {
    pub method: String,
    pub http_version: String,
    pub path: String,
    pub query_string: String,
    pub headers: Vec<(Bytes, Bytes)>,
    pub body: Bytes,
    pub client_host: String,
    pub client_port: u16,
    pub server_host: String,
    pub server_port: u16,
}

#[derive(Debug, Clone)]
pub struct AppResponse {
    pub status: u16,
    pub headers: Vec<(Bytes, Bytes)>,
    pub body: Bytes,
}

struct PythonHelpers {
    asgi_handle_raw: Py<PyAny>,
    wsgi_handle_raw: Py<PyAny>,
}

pub struct AppState {
    interface: Interface,
    workers: usize,
    app: Py<PyAny>,
    helpers: PythonHelpers,
}

impl AppState {
    pub fn new(cli: &Cli) -> Result<Self> {
        Python::with_gil(|py| -> PyResult<Self> {
            patch_python_path(py, cli)?;

            let app = load_target(py, &cli.app)?;
            let helpers_module = PyModule::from_code_bound(
                py,
                HELPERS_CODE,
                "_kubstu_helpers.py",
                "_kubstu_helpers",
            )?;
            let helpers = PythonHelpers {
                asgi_handle_raw: helpers_module.getattr("_rust_asgi_handle_raw")?.unbind(),
                wsgi_handle_raw: helpers_module.getattr("_rust_wsgi_handle_raw")?.unbind(),
            };

            Ok(Self {
                interface: cli.interface,
                workers: cli.workers,
                app,
                helpers,
            })
        })
        .map_err(pyerr_to_anyhow)
    }

    pub fn execute(&self, request: RequestData) -> Result<AppResponse> {
        match self.interface {
            Interface::Asgi => self.execute_asgi(request),
            Interface::Wsgi => self.execute_wsgi(request),
        }
    }

    fn execute_asgi(&self, request: RequestData) -> Result<AppResponse> {
        Python::with_gil(|py| -> PyResult<AppResponse> {
            let py_headers = build_py_headers(py, &request.headers)?;
            let body = PyBytes::new_bound(py, request.body.as_ref());
            let result = self.helpers.asgi_handle_raw.bind(py).call1((
                self.app.bind(py),
                request.method.as_str(),
                request.http_version.as_str(),
                request.path.as_str(),
                request.query_string.as_str(),
                py_headers,
                body,
                request.client_host.as_str(),
                request.client_port,
                request.server_host.as_str(),
                request.server_port,
            ))?;

            let (status, headers, body): RawAsgiResponse = result.extract()?;
            let headers = headers
                .into_iter()
                .map(|(name, value)| (Bytes::from(name), Bytes::from(value)))
                .collect();

            Ok(AppResponse {
                status,
                headers,
                body: Bytes::from(body),
            })
        })
        .map_err(pyerr_to_anyhow)
    }

    fn execute_wsgi(&self, request: RequestData) -> Result<AppResponse> {
        Python::with_gil(|py| -> PyResult<AppResponse> {
            let py_headers = build_py_headers(py, &request.headers)?;
            let body = PyBytes::new_bound(py, request.body.as_ref());
            let result = self.helpers.wsgi_handle_raw.bind(py).call1((
                self.app.bind(py),
                request.method.as_str(),
                request.http_version.as_str(),
                request.path.as_str(),
                request.query_string.as_str(),
                py_headers,
                body,
                request.client_host.as_str(),
                request.client_port,
                request.server_host.as_str(),
                request.server_port,
                self.workers > 1,
            ))?;

            let (status_line, headers, body): (String, Vec<(String, String)>, Vec<u8>) =
                result.extract()?;
            let status = parse_wsgi_status(&status_line);
            let headers = headers
                .into_iter()
                .map(|(name, value)| {
                    (
                        Bytes::from(name.into_bytes()),
                        Bytes::from(value.into_bytes()),
                    )
                })
                .collect();

            Ok(AppResponse {
                status,
                headers,
                body: Bytes::from(body),
            })
        })
        .map_err(pyerr_to_anyhow)
    }
}

fn build_py_headers<'py>(
    py: Python<'py>,
    headers: &[(Bytes, Bytes)],
) -> PyResult<Bound<'py, PyList>> {
    let py_headers = PyList::empty_bound(py);
    for (name, value) in headers {
        py_headers.append((
            PyBytes::new_bound(py, name.as_ref()),
            PyBytes::new_bound(py, value.as_ref()),
        ))?;
    }
    Ok(py_headers)
}

fn pyerr_to_anyhow(err: PyErr) -> anyhow::Error {
    Python::with_gil(|py| {
        err.print(py);
    });

    anyhow!(err.to_string())
}

fn parse_wsgi_status(status_line: &str) -> u16 {
    status_line
        .split_whitespace()
        .next()
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or(500)
}

fn patch_python_path(py: Python<'_>, cli: &Cli) -> PyResult<()> {
    let sys = py.import_bound("sys")?;
    let py_path = sys.getattr("path")?;

    if let Some(wd) = &cli.working_dir {
        py_path.call_method1("insert", (0, wd.to_string_lossy().to_string()))?;
    }

    if let Ok(venv) = std::env::var("VIRTUAL_ENV") {
        let version_info = sys.getattr("version_info")?;
        let major = version_info.get_item(0)?.extract::<u8>()?;
        let minor = version_info.get_item(1)?.extract::<u8>()?;
        let site = py.import_bound("site")?;

        let candidates = [
            format!("{venv}/lib/python{major}.{minor}/site-packages"),
            format!("{venv}/lib64/python{major}.{minor}/site-packages"),
            format!("{venv}/Lib/site-packages"),
        ];

        for candidate in candidates {
            if Path::new(&candidate).exists() {
                site.call_method1("addsitedir", (candidate,))?;
            }
        }
    }

    Ok(())
}

fn load_target(py: Python<'_>, target: &str) -> PyResult<Py<PyAny>> {
    let (module_name, attr_path) = split_target(target);

    let importlib = py.import_bound("importlib")?;
    let module = importlib.call_method1("import_module", (module_name,))?;

    let mut obj = module;
    for attr in attr_path.split('.') {
        obj = obj.getattr(attr)?;
    }

    if !obj.is_callable() {
        return Err(PyTypeError::new_err(format!(
            "Target '{}' resolved to a non-callable object",
            target
        )));
    }

    Ok(obj.unbind())
}

fn split_target(target: &str) -> (&str, &str) {
    match target.split_once(':') {
        Some((module, attr)) if !attr.trim().is_empty() => (module, attr),
        Some((module, _)) => (module, "app"),
        None => (target, "app"),
    }
}
