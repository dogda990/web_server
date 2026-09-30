use clap::{Parser, ValueEnum};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Interface {
    Asgi,
    Wsgi,
}

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum ListenerMode {
    Shared,
    Reuseport,
}

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum PythonMode {
    Inline,
    Spawn,
    Blocking,
    Pool,
}

impl Interface {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Asgi => "asgi",
            Self::Wsgi => "wsgi",
        }
    }
}

#[derive(Debug, Clone, Parser)]
#[command(
    name = "kubstu-web-server",
    version,
    about = "MVP Rust web server for Django ASGI/WSGI"
)]
pub struct Cli {
    #[arg(
        help = "Application target in module:callable form (for example project.asgi:application)"
    )]
    pub app: String,

    #[arg(long, value_enum, default_value_t = Interface::Asgi)]
    pub interface: Interface,

    #[arg(long, default_value = "127.0.0.1")]
    pub host: String,

    #[arg(long, default_value_t = 8000)]
    pub port: u16,

    #[arg(long, default_value_t = 1)]
    pub workers: usize,

    #[arg(long, hide = true, default_value_t = false)]
    pub force_master: bool,

    #[arg(long, value_enum, default_value_t = ListenerMode::Shared)]
    pub listener_mode: ListenerMode,

    #[arg(
        long,
        default_value_t = 1,
        help = "Tokio runtime worker threads per process"
    )]
    pub runtime_threads: usize,

    #[arg(
        long,
        default_value_t = 4,
        help = "Number of dedicated Python execution threads"
    )]
    pub python_threads: usize,

    #[arg(long, value_enum, default_value_t = PythonMode::Pool)]
    pub python_mode: PythonMode,

    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    pub bounded_python_queue: bool,

    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    pub compact_body_buffer: bool,

    #[arg(
        long,
        default_value_t = 256,
        help = "Maximum queued Python requests per process; excess requests receive HTTP 503"
    )]
    pub python_queue_capacity: usize,

    #[arg(long, default_value_t = 8 * 1024 * 1024, help = "Maximum request body size in bytes; larger bodies receive HTTP 413")]
    pub max_body_bytes: usize,

    #[arg(long, help = "Working directory to prepend to sys.path")]
    pub working_dir: Option<PathBuf>,

    #[arg(long, hide = true, default_value_t = false)]
    pub worker_child: bool,
}

impl Cli {
    pub fn parse_args() -> Self {
        Self::parse()
    }
}
