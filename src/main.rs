use clap::Parser;
use serde::Deserialize;
use std::fs;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;
use tracing::{debug, info, span, Level};
use tracing_subscriber;

use htraffic::Connection;

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
    pub key: Option<String>,
    pub cert: Option<String>,
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
        let n = params.connections.expect("connections param must exist");
        let tps = params.tps.expect("tps param must exist");
        let workers = params.workers.expect("workers param must exist");

        // TODO: improve this
        let pem = if let Some(key) = params.key {
            if let Some(cert) = params.cert {
                let key = fs::read_to_string(key).expect("should read the whole key file");
                let cert = fs::read_to_string(cert).expect("should read the whole cert file");
                let pem = format!("{}\n{}", cert.trim(), key.trim());
                info!("got pem file: {}", pem);
                Some(pem)
            } else {
                None
            }
        } else {
            None
        };

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
                let pem = pem.clone();
                let ah = connections.spawn(async move {
                    let mut conn = Connection::new(conn_id, &url, wrx, workers, pem);
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
