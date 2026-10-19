use std::{
    fmt,
    future::IntoFuture,
    net::SocketAddr,
    sync::{Arc, mpsc},
    thread,
    time::Duration,
};

use axum::{Router, routing::any};
use tokio::{net::TcpListener, runtime, sync::oneshot};

use crate::{Error, MusicSource, Result, api};

/// How long `stop` lets in-flight transfers finish before abandoning them.
const GRACE: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub struct ServerConfig {
    pub bind: SocketAddr,
    pub username: String,
    pub password: String,
    /// Enables OpenSubsonic `apiKey` authentication when set.
    pub api_key: Option<String>,
}

impl fmt::Debug for ServerConfig {
    /// The credentials stay out of `{:?}` so a logged config never leaks them.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServerConfig")
            .field("bind", &self.bind)
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

pub struct AppState {
    pub config: ServerConfig,
    pub source: Arc<dyn MusicSource>,
}

/// A running Subsonic server. Dropping it shuts it down without waiting; [`Server::stop`] waits.
pub struct Server {
    addr: SocketAddr,
    /// Dropping the sender is itself the stop signal, so a dropped `Server` cleans up too.
    shutdown: Option<oneshot::Sender<()>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Server {
    /// Binds `config.bind` and returns once the server is listening. The server owns a thread with
    /// its own async runtime, so callers need neither.
    pub fn start(config: ServerConfig, source: Arc<dyn MusicSource>) -> Result<Self> {
        if config.username.is_empty() || config.password.is_empty() {
            return Err(Error::InvalidConfig(
                "username and password must not be empty",
            ));
        }
        let (ready_tx, ready_rx) = mpsc::channel();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let state = Arc::new(AppState { config, source });
        let thread = thread::Builder::new()
            .name("subsonic-server".to_owned())
            .spawn(move || run(state, &ready_tx, shutdown_rx))
            .map_err(Error::Start)?;

        match ready_rx.recv() {
            Ok(Ok(addr)) => Ok(Self {
                addr,
                shutdown: Some(shutdown_tx),
                thread: Some(thread),
            }),
            Ok(Err(err)) => {
                let _ = thread.join();
                Err(err)
            }
            Err(_) => Err(Error::Start(std::io::Error::other(
                "server thread exited before listening",
            ))),
        }
    }

    /// The bound address; useful when `bind` asked for port 0.
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// Stops accepting connections, lets in-flight requests finish (up to a grace period), and
    /// waits for the server thread to exit.
    pub fn stop(mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run(
    state: Arc<AppState>,
    ready: &mpsc::Sender<Result<SocketAddr>>,
    shutdown: oneshot::Receiver<()>,
) {
    // Request handling is I/O-bound and source calls run on the blocking pool, so two workers are plenty.
    let runtime = match runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("subsonic-worker")
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            let _ = ready.send(Err(Error::Start(err)));
            return;
        }
    };

    runtime.block_on(async {
        let listener = match TcpListener::bind(state.config.bind).await {
            Ok(listener) => listener,
            Err(err) => {
                let _ = ready.send(Err(Error::Start(err)));
                return;
            }
        };
        let Ok(addr) = listener.local_addr() else {
            return;
        };
        let _ = ready.send(Ok(addr));

        let app = Router::new()
            .route("/rest/{name}", any(api::handle))
            .with_state(state);
        let (stopping_tx, stopping_rx) = oneshot::channel::<()>();
        let serve = tokio::spawn(
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = shutdown.await;
                    let _ = stopping_tx.send(());
                })
                .into_future(),
        );
        let _ = stopping_rx.await;
        // Past the grace period the runtime shutdown below cancels whatever is still streaming.
        let _ = tokio::time::timeout(GRACE, serve).await;
    });
    runtime.shutdown_timeout(Duration::from_secs(1));
}
