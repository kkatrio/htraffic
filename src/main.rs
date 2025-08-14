use clap::Parser;
use reqwest::{Client, ClientBuilder};
use serde::Deserialize;
use tokio::sync::{mpsc, watch};
use tokio::task::{JoinHandle, JoinSet};
use tokio::time;
use tracing::{debug, info, span, Level};
use tracing_subscriber;

mod api_server;
use api_server::start_api_server;

#[derive(Parser, Debug)]
#[command(author, version, about)]
pub struct Config {
    /// The URL to send HTTP/2 traffic to
    #[arg(short, long)]
    pub path: String,
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct Params {
    pub connections: Option<usize>,
    pub tps: Option<u64>,
    pub workers: Option<usize>,
}

struct Connection {
    connection_id: usize,
    workers: Vec<Worker>,
}

struct Worker {
    pub task: JoinHandle<()>,
}

impl Worker {
    fn new(id: String, url: &'static str, client: Client, mut wrx: watch::Receiver<u64>) -> Worker {
        let task = tokio::spawn(async move {
            let mut intv = *wrx.borrow();
            let mut interval = time::interval(time::Duration::from_micros(intv));
            loop {
                tokio::select! {
                    _ = wrx.changed() => {
                        intv = *wrx.borrow();
                        interval = time::interval(time::Duration::from_micros(intv));
                        debug!("interval changed, duration: {} ms", interval.period().as_millis());
                    }
                    _ = interval.tick() => {
                        debug!("sending from worker {}", id);
                        // TODO: step counter here
                        let resp = client.get(url).send().await.unwrap();
                        let _body = resp.bytes().await.unwrap();
                        // TODO: step counter here
                    }
                }
            }
        });

        Worker { task }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        info!("dropping worker...");
        self.task.abort();
    }
}

impl Connection {
    // should watch_rx be a reference here?
    fn new(
        id: usize,
        url: &'static str,
        watch_rx: watch::Receiver<u64>,
        number_of_workers: usize,
    ) -> Connection {
        let client = ClientBuilder::new()
            .http2_prior_knowledge()
            .use_rustls_tls()
            .build()
            .unwrap();

        let mut workers = Vec::with_capacity(number_of_workers);
        for i in 0..number_of_workers {
            let client = client.clone();
            let watch_rx = watch_rx.clone();
            let worker_id = format!("{}-{}", id.to_string(), i.to_string());
            workers.push(Worker::new(worker_id, url, client, watch_rx))
        }

        Self {
            connection_id: id,
            workers: workers,
        }
    }

    async fn wait_workers(&mut self) {
        info!(
            "awaiting workers for connection with id: {}",
            self.connection_id
        );
        let mut task_handles = Vec::with_capacity(self.workers.len());
        for worker in &mut self.workers {
            let task = &mut worker.task;
            task_handles.push(task.await.unwrap());
        }
        unreachable!()
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    let config = Config::parse();

    let url: &'static str = Box::leak(config.path.into_boxed_str());
    // pass Params received by the api server to main thread
    let (tx, mut rx) = mpsc::channel(32);
    // store connection handles
    let mut connections = JoinSet::new();
    let mut abort_handles = Vec::<tokio::task::AbortHandle>::new();
    // identify each connection
    let mut conn_id = 0;

    // pass interval change to all workers
    // an initial value does nothing, since the connections/workers are created when we receive the
    // tps param.
    let (wtx, wrx) = watch::channel(0);

    // spawn the api server
    tokio::spawn(start_api_server(tx.clone()));

    let span = span!(Level::INFO, "main");
    let _guard = span.enter();

    // blocks the main thread
    while let Some(params) = rx.recv().await {
        let n = params.connections.expect("connections param should exist");
        let tps = params.tps.expect("tps param should exist");
        let workers = params.workers.expect("workers param should exist");
        // TODO: handle None -- if the query does not include parameter
        info!(
            "received: connections: {}, tps: {}, workers: {}",
            n, tps, workers
        );

        debug!("start -- receivers: {}", wtx.receiver_count());

        // tps to interval
        let interval = if tps != 0 { 1000000 / tps } else { u64::MAX };
        // wpli traffic to workers, each worker waits more
        let interval = interval * workers as u64;
        info!("interval per worker: {} micros", interval);

        // sending tps to all workers
        wtx.send(interval).unwrap();

        if n == conn_id {
            continue;
        } else if n > conn_id {
            let new = n - conn_id;
            for _ in 0..new {
                conn_id += 1;
                let wrx = wrx.clone();
                let ah = connections.spawn(async move {
                    let mut conn = Connection::new(conn_id, &url, wrx, workers);
                    conn.wait_workers().await;
                });
                abort_handles.push(ah);
                debug!(
                    "added connection: active connections: {} -- receivers: {}",
                    abort_handles.len(),
                    wtx.receiver_count()
                )
            }
        } else {
            // dropping connections
            let drop_n = conn_id - n;
            for _ in 0..drop_n {
                conn_id -= 1;
                if let Some(ah) = abort_handles.pop() {
                    ah.abort();
                    debug!(
                        "aborted connection: active connections: {} -- receivers: {}",
                        abort_handles.len(),
                        wtx.receiver_count()
                    )
                }
            }
        }
    }
}
