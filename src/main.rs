mod cli;
mod python_bridge;
mod server;

use anyhow::Context;
use anyhow::Result;
#[cfg(unix)]
use std::os::fd::AsRawFd;
use std::sync::Arc;
use std::{env, process::Command};

use crate::cli::{Cli, ListenerMode};
use crate::python_bridge::AppState;

fn main() -> Result<()> {
    let cli = Cli::parse_args();
    if (cli.workers > 1 || cli.force_master) && !cli.worker_child {
        run_master(&cli)?;
        return Ok(());
    }

    let state = Arc::new(AppState::new(&cli)?);
    let runtime_threads = cli.runtime_threads.max(1);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(runtime_threads)
        .thread_name("kubstu-tokio")
        .build()?;

    runtime.block_on(server::serve(
        cli.clone(),
        state,
        cli.listener_mode == ListenerMode::Reuseport,
    ))
}

fn run_master(cli: &Cli) -> Result<()> {
    let exe = env::current_exe()?;
    let mut children = Vec::with_capacity(cli.workers);
    #[cfg(unix)]
    let shared_listener = if cli.listener_mode == ListenerMode::Shared {
        Some(
            server::bind_std_listener(cli.host.as_str(), cli.port, false).with_context(|| {
                format!("Failed to bind shared listener {}:{}", cli.host, cli.port)
            })?,
        )
    } else {
        None
    };

    for idx in 0..cli.workers {
        let mut cmd = Command::new(&exe);
        cmd.arg("--interface")
            .arg(cli.interface.as_str())
            .arg("--host")
            .arg(&cli.host)
            .arg("--port")
            .arg(cli.port.to_string())
            .arg("--runtime-threads")
            .arg(cli.runtime_threads.to_string())
            .arg("--python-threads")
            .arg(cli.python_threads.to_string())
            .arg("--python-queue-capacity")
            .arg(cli.python_queue_capacity.to_string())
            .arg("--max-body-bytes")
            .arg(cli.max_body_bytes.to_string())
            .arg("--workers")
            .arg(cli.workers.to_string())
            .arg("--listener-mode")
            .arg(match cli.listener_mode {
                ListenerMode::Shared => "shared",
                ListenerMode::Reuseport => "reuseport",
            })
            .arg("--python-mode")
            .arg(match cli.python_mode {
                crate::cli::PythonMode::Inline => "inline",
                crate::cli::PythonMode::Spawn => "spawn",
                crate::cli::PythonMode::Blocking => "blocking",
                crate::cli::PythonMode::Pool => "pool",
            })
            .arg("--bounded-python-queue")
            .arg(cli.bounded_python_queue.to_string())
            .arg("--compact-body-buffer")
            .arg(cli.compact_body_buffer.to_string())
            .arg("--worker-child");

        if let Some(wd) = &cli.working_dir {
            cmd.arg("--working-dir").arg(wd);
        }

        cmd.arg(&cli.app);
        cmd.env("KUBSTU_WORKER_ID", (idx + 1).to_string());

        #[cfg(unix)]
        let child = if let Some(shared_listener) = &shared_listener {
            let child_listener = shared_listener
                .try_clone()
                .with_context(|| "Failed to clone shared listener for child process")?;
            let fd = child_listener.as_raw_fd();
            clear_cloexec(fd)?;
            cmd.env("KUBSTU_LISTENER_FD", fd.to_string());
            let spawned = cmd.spawn();
            set_cloexec(fd)?;
            drop(child_listener);
            spawned?
        } else {
            cmd.spawn()?
        };

        #[cfg(not(unix))]
        let child = cmd.spawn()?;

        println!("Spawned worker-{} pid={}", idx + 1, child.id());
        children.push((idx + 1, child));
    }

    for (idx, mut child) in children {
        let status = child.wait()?;
        println!("Worker-{idx} exited with status: {status}");
    }

    Ok(())
}

#[cfg(unix)]
fn clear_cloexec(fd: i32) -> Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("fcntl(F_GETFD) failed for fd {fd}"));
    }

    let next_flags = flags & !libc::FD_CLOEXEC;
    if unsafe { libc::fcntl(fd, libc::F_SETFD, next_flags) } < 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("fcntl(F_SETFD clear CLOEXEC) failed for fd {fd}"));
    }

    Ok(())
}

#[cfg(unix)]
fn set_cloexec(fd: i32) -> Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("fcntl(F_GETFD) failed for fd {fd}"));
    }

    let next_flags = flags | libc::FD_CLOEXEC;
    if unsafe { libc::fcntl(fd, libc::F_SETFD, next_flags) } < 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("fcntl(F_SETFD set CLOEXEC) failed for fd {fd}"));
    }

    Ok(())
}
