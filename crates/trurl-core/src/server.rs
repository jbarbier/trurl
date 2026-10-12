//! Starts, watches and stops llama-server.
//!
//! One server runs at a time, bound to 127.0.0.1 on a free port, with a random
//! API key so no other program on the machine can use it. It never outlives
//! Trurl: on Windows it is placed in a Job Object that kills it when Trurl's
//! last handle closes (including a crash); on Linux it gets PR_SET_PDEATHSIG,
//! set from one long-lived thread because the signal follows the spawning
//! *thread*, not the process.

use crate::errors::UserError;
use crate::hardware::CpuVariant;
use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const LOG_LINES: usize = 300;

/// Everything about how llama-server runs. Only `context` is a learner control;
/// the rest is fixed for low-RAM laptops.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerConfig {
    pub models_dir: PathBuf,
    pub model_file: String,
    pub context: u32,
    pub threads: usize,
    pub variant: CpuVariant,
}

impl ServerConfig {
    pub fn args(&self, port: u16, api_key: &str) -> Vec<String> {
        vec![
            // Relative to the working directory (the models folder): keeps
            // non-ASCII user names out of the command line.
            "-m".into(),
            self.model_file.clone(),
            "-c".into(),
            self.context.to_string(),
            "-t".into(),
            self.threads.to_string(),
            // One conversation at a time: one slot, no spare KV cache.
            "-np".into(),
            "1".into(),
            // The default prompt cache is 8 GB of RAM: far too much on a 4 GB laptop.
            "--cache-ram".into(),
            "0".into(),
            // We set the context ourselves; do not let llama.cpp shrink or grow it.
            "--fit".into(),
            "off".into(),
            "--jinja".into(),
            "--reasoning-format".into(),
            "deepseek".into(),
            "--no-webui".into(),
            "--offline".into(),
            "--host".into(),
            "127.0.0.1".into(),
            "--port".into(),
            port.to_string(),
            "--api-key".into(),
            api_key.to_string(),
        ]
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExitKind {
    /// The CPU does not have an instruction the build needs (AVX2 build on an old CPU).
    IllegalInstruction,
    /// Someone else took the port between our check and the server's bind.
    PortInUse,
    TooBig,
    Corrupt,
    Unsupported,
    Other,
}

/// Read the end of the server log and the exit status, and say what went wrong.
pub fn classify_exit(log: &str, code: Option<i32>) -> ExitKind {
    const ILLEGAL_WIN: i32 = 0xC000001Du32 as i32;
    if code == Some(ILLEGAL_WIN) || code == Some(-4) || code == Some(128 + 4) {
        return ExitKind::IllegalInstruction;
    }
    let l = log.to_ascii_lowercase();
    if l.contains("couldn't bind") || l.contains("address already in use") || l.contains("only one usage of each socket") {
        ExitKind::PortInUse
    } else if l.contains("unknown model architecture") || l.contains("unsupported model architecture") {
        ExitKind::Unsupported
    } else if l.contains("failed to allocate")
        || l.contains("unable to allocate")
        || l.contains("out of memory")
        || l.contains("bad_alloc")
        || l.contains("cannot allocate memory")
        || l.contains("failed to create context")
        || l.contains("not enough memory")
        || l.contains("failed to mmap")
    {
        ExitKind::TooBig
    } else if l.contains("invalid magic")
        || l.contains("not within the file bounds")
        || l.contains("failed to read")
        || l.contains("gguf_init_from_file")
        || l.contains("corrupted")
        || l.contains("invalid gguf")
        || l.contains("failed to load model")
    {
        ExitKind::Corrupt
    } else {
        ExitKind::Other
    }
}

pub fn exit_error(kind: &ExitKind, model_name: &str, log_tail: &str) -> UserError {
    let e = match kind {
        ExitKind::TooBig => UserError::model_too_big(model_name),
        ExitKind::Corrupt => UserError::model_corrupt(model_name),
        ExitKind::Unsupported => UserError::model_unsupported(model_name),
        _ => UserError::server_start(),
    };
    e.with_detail(log_tail.to_string())
}

#[derive(Clone, Default)]
pub struct LogRing(Arc<Mutex<VecDeque<String>>>);

impl LogRing {
    fn push(&self, line: String) {
        let mut q = self.0.lock().unwrap();
        if q.len() == LOG_LINES {
            q.pop_front();
        }
        q.push_back(line);
    }
    pub fn tail(&self, n: usize) -> String {
        let q = self.0.lock().unwrap();
        let skip = q.len().saturating_sub(n);
        q.iter().skip(skip).cloned().collect::<Vec<_>>().join("\n")
    }
}

pub struct Server {
    child: Child,
    pub port: u16,
    pub api_key: String,
    pub config: ServerConfig,
    pub log: LogRing,
}

impl Server {
    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// Spawn the process. Does not wait for the model to load: see `wait_ready`.
    pub fn spawn(cfg: &ServerConfig, llama_root: &Path) -> Result<Server, UserError> {
        let dir = llama_root.join(cfg.variant.dir_name());
        let exe = dir.join(crate::paths::server_exe_name());
        if !exe.is_file() {
            return Err(UserError::server_missing().with_detail(exe.display().to_string()));
        }
        if !cfg.models_dir.join(&cfg.model_file).is_file() {
            return Err(UserError::model_missing(&cfg.model_file));
        }
        let port = free_port().map_err(|e| UserError::server_start().with_detail(e.to_string()))?;
        let api_key = random_key();
        let mut cmd = Command::new(&exe);
        cmd.args(cfg.args(port, &api_key))
            .current_dir(&cfg.models_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // llama-server must never reach the network or pick up the user's
            // HF cache or proxy settings.
            .env_remove("HF_TOKEN")
            .env_remove("LLAMA_ARG_HF_REPO")
            .env("LLAMA_OFFLINE", "1");
        // The libraries sit next to the exe; make sure the loader finds them.
        #[cfg(target_os = "linux")]
        {
            let mut ld = dir.as_os_str().to_owned();
            if let Some(old) = std::env::var_os("LD_LIBRARY_PATH") {
                ld.push(":");
                ld.push(old);
            }
            cmd.env("LD_LIBRARY_PATH", ld);
        }
        let mut child = platform::spawn(cmd).map_err(|e| UserError::server_start().with_detail(e.to_string()))?;
        let log = LogRing::default();
        for stream in [child.stdout.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>), child.stderr.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>)].into_iter().flatten() {
            let log = log.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stream).lines() {
                    match line {
                        Ok(l) => log.push(l),
                        Err(_) => break,
                    }
                }
            });
        }
        Ok(Server { child, port, api_key, config: cfg.clone(), log })
    }

    /// Exit code if the process has ended.
    pub fn exited(&mut self) -> Option<Option<i32>> {
        match self.child.try_wait() {
            Ok(Some(status)) => Some(exit_code(&status)),
            Ok(None) => None,
            Err(_) => Some(None),
        }
    }

    /// Poll `/health` until the model is loaded. No time limit: a 4B model on a
    /// slow disk can take minutes. Returns the exit kind if the process died.
    pub async fn wait_ready(&mut self, http: &reqwest::Client, cancel: &tokio_util::sync::CancellationToken) -> Result<(), ExitKind> {
        let url = format!("{}/health", self.base_url());
        loop {
            if cancel.is_cancelled() {
                return Err(ExitKind::Other);
            }
            if let Some(code) = self.exited() {
                // Let the reader threads drain the last lines.
                tokio::time::sleep(Duration::from_millis(150)).await;
                return Err(classify_exit(&self.log.tail(80), code));
            }
            // Refused (not listening yet) and 503 (loading) both mean "keep waiting".
            if let Ok(r) = http.get(&url).timeout(Duration::from_secs(2)).send().await {
                if r.status().is_success() {
                    return Ok(());
                }
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(250)) => {}
                _ = cancel.cancelled() => return Err(ExitKind::Other),
            }
        }
    }

    /// Stop the server and wait until the process is gone (so its files are released).
    pub fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop();
    }
}

fn exit_code(status: &std::process::ExitStatus) -> Option<i32> {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return Some(-sig);
        }
    }
    status.code()
}

fn free_port() -> std::io::Result<u16> {
    let l = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    Ok(l.local_addr()?.port())
}

fn random_key() -> String {
    use rand::RngExt;
    let bytes: [u8; 16] = rand::rng().random();
    hex::encode(bytes)
}

#[cfg(windows)]
mod platform {
    use std::os::windows::process::CommandExt;
    use std::process::{Child, Command};
    use std::sync::OnceLock;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::JobObjects::*;
    use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

    struct Job(HANDLE);
    // SAFETY: the job handle is only used through thread-safe Win32 calls.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    /// One job for the life of Trurl. The handle is never closed by us: when
    /// Trurl exits for any reason, Windows closes it and kills every child.
    fn job() -> Option<&'static Job> {
        static JOB: OnceLock<Option<Job>> = OnceLock::new();
        JOB.get_or_init(|| unsafe {
            let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if h.is_null() {
                return None;
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                h,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok == 0 { None } else { Some(Job(h)) }
        })
        .as_ref()
    }

    pub fn spawn(mut cmd: Command) -> std::io::Result<Child> {
        // A GUI app spawning a console program opens a console window unless told not to.
        cmd.creation_flags(CREATE_NO_WINDOW);
        let child = cmd.spawn()?;
        if let Some(j) = job() {
            use std::os::windows::io::AsRawHandle;
            // SAFETY: both handles are valid for the duration of the call.
            unsafe {
                AssignProcessToJobObject(j.0, child.as_raw_handle() as HANDLE);
            }
        }
        Ok(child)
    }
}

#[cfg(unix)]
mod platform {
    use std::process::{Child, Command};
    use std::sync::{Mutex, OnceLock, mpsc};

    type Req = (Command, mpsc::Sender<std::io::Result<Child>>);

    /// PR_SET_PDEATHSIG fires when the *thread* that forked the child exits. Tokio
    /// retires idle threads, so every spawn happens on this thread, which lives
    /// as long as the process.
    fn keeper() -> &'static Mutex<mpsc::Sender<Req>> {
        static K: OnceLock<Mutex<mpsc::Sender<Req>>> = OnceLock::new();
        K.get_or_init(|| {
            let (tx, rx) = mpsc::channel::<Req>();
            std::thread::Builder::new()
                .name("trurl-spawner".into())
                .spawn(move || {
                    for (mut cmd, reply) in rx {
                        let _ = reply.send(spawn_here(&mut cmd));
                    }
                })
                .expect("spawner thread");
            Mutex::new(tx)
        })
    }

    fn spawn_here(cmd: &mut Command) -> std::io::Result<Child> {
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::process::CommandExt;
            let parent = std::process::id() as libc::pid_t;
            // SAFETY: only async-signal-safe calls between fork and exec.
            unsafe {
                cmd.pre_exec(move || {
                    if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    // The parent may have died before prctl took effect.
                    if libc::getppid() != parent {
                        libc::_exit(1);
                    }
                    Ok(())
                });
            }
        }
        cmd.spawn()
    }

    pub fn spawn(cmd: Command) -> std::io::Result<Child> {
        let (tx, rx) = mpsc::channel();
        keeper().lock().unwrap().send((cmd, tx)).map_err(|_| std::io::Error::other("spawner gone"))?;
        rx.recv().map_err(|_| std::io::Error::other("spawner gone"))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_are_the_low_ram_defaults() {
        let c = ServerConfig {
            models_dir: "/m".into(),
            model_file: "gemma.gguf".into(),
            context: 2048,
            threads: 4,
            variant: CpuVariant::Avx2,
        };
        let a = c.args(5555, "k").join(" ");
        assert_eq!(
            a,
            "-m gemma.gguf -c 2048 -t 4 -np 1 --cache-ram 0 --fit off --jinja --reasoning-format deepseek --no-webui --offline --host 127.0.0.1 --port 5555 --api-key k"
        );
    }

    #[test]
    fn classifies_real_failures() {
        assert_eq!(classify_exit("", Some(0xC000001Du32 as i32)), ExitKind::IllegalInstruction);
        assert_eq!(classify_exit("", Some(-4)), ExitKind::IllegalInstruction);
        assert_eq!(classify_exit("error: couldn't bind HTTP server socket, hostname: 127.0.0.1, port: 5", Some(1)), ExitKind::PortInUse);
        assert_eq!(classify_exit("llama_model_load: error loading model: error loading model architecture: unknown model architecture: 'foo'", Some(1)), ExitKind::Unsupported);
        assert_eq!(classify_exit("ggml_backend_cpu_buffer_type_alloc_buffer: failed to allocate buffer of size 4294967296", Some(1)), ExitKind::TooBig);
        assert_eq!(classify_exit("gguf_init_from_file_impl: invalid magic characters: 'PK'", Some(1)), ExitKind::Corrupt);
        assert_eq!(classify_exit("tensor 'blk.3.ffn' data is not within the file bounds, model is corrupted or incomplete", Some(1)), ExitKind::Corrupt);
        assert_eq!(classify_exit("something odd", Some(1)), ExitKind::Other);
    }

    #[test]
    fn log_ring_keeps_the_tail() {
        let r = LogRing::default();
        for i in 0..(LOG_LINES + 10) {
            r.push(format!("l{i}"));
        }
        let t = r.tail(2);
        assert_eq!(t, format!("l{}\nl{}", LOG_LINES + 8, LOG_LINES + 9));
    }

    #[test]
    fn missing_binary_is_a_plain_error() {
        let d = tempfile::tempdir().unwrap();
        let c = ServerConfig { models_dir: d.path().into(), model_file: "x.gguf".into(), context: 256, threads: 1, variant: CpuVariant::Compat };
        let Err(e) = Server::spawn(&c, d.path()) else { panic!("spawned") };
        assert_eq!(e.code, crate::errors::ErrorCode::ServerMissing);
    }
}
