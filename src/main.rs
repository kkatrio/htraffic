use clap::Parser;
use reqwest::{Client, ClientBuilder};
use serde::Deserialize;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tokio::task::{JoinHandle, JoinSet};
use tokio::time;
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
}

struct Connection {
    connection_id: usize,
    interval: Arc<Mutex<u64>>,
    workers: Vec<Worker>,
}

struct Worker {
    pub task: JoinHandle<()>,
}

impl Worker {
    fn new(id: String, url: &'static str, client: Client, interval: Arc<Mutex<u64>>) -> Worker {
        let intv = {
            let guard = interval.lock().unwrap();
            *guard
        };
        println!("worker id: {}, ntv: {}", id, intv);

        let mut interval = time::interval(time::Duration::from_micros(intv));

        let task = tokio::spawn(async move {
            loop {
                interval.tick().await;
                println!("sending from worker with id {}", id);
                let resp = client.get(url).send().await.unwrap();
                let _body = resp.bytes().await.unwrap();
            }
        });

        Worker { task }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        println!("dropping worker...");
        self.task.abort();
    }
}

impl Connection {
    fn new(id: usize, interval: Arc<Mutex<u64>>, url: &'static str) -> Connection {
        let client = ClientBuilder::new()
            .http2_prior_knowledge()
            .use_rustls_tls()
            .connection_verbose(true)
            .build()
            .unwrap();

        let mut workers = Vec::with_capacity(2);
        for i in 0..2 {
            let client = client.clone();
            let interval = interval.clone();
            let worker_id = format!("{}-{}", id.to_string(), i.to_string());
            workers.push(Worker::new(worker_id, url, client, interval))
        }

        Self {
            connection_id: id,
            interval: interval,
            workers: workers,
        }
    }

    async fn wait_workers(&mut self) {
        println!("awaiting workers for connection: {}", self.connection_id);
        let mut task_handles = Vec::with_capacity(self.workers.len());
        for worker in &mut self.workers {
            let task = &mut worker.task;
            task_handles.push(task.await.unwrap());
        }
        unreachable!()
    }

    //async fn send(&self, notify: Arc<Notify>) {
    //    println!("sending HTTP/2.0 from connection: {}", self.connection_id);

    //    let mut intv = {
    //        let guard = self.interval.lock().unwrap();
    //        *guard
    //    };
    //    let mut interval = time::interval(time::Duration::from_micros(intv));

    //    loop {
    //        let new_intv = {
    //            let guard = self.interval.lock().unwrap();
    //            *guard
    //        };
    //        if new_intv != intv {
    //            println!("changing interval");
    //            intv = new_intv;
    //            interval = time::interval(time::Duration::from_micros(intv));
    //        }

    //        tokio::select! {
    //            _ = notify.notified() => {
    //                println!("notification received -- dropping connection");
    //                break;
    //            }
    //            _ = interval.tick() => {
    //                //println!("period: {}, tick at: {:?}",  interval.period().as_micros(), Instant::now());
    //                println!("send from multiple?");
    //                self.send_from_all_workers().await;
    //            }
    //        }
    //    }
    //}
}

fn update_interval(interval: &Arc<Mutex<u64>>, mps: u64) {
    let mut iguard = interval.lock().unwrap();
    *iguard = if mps != 0 { 1000000 / mps } else { u64::MAX };
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    let config = Config::parse();

    let url: &'static str = Box::leak(config.path.into_boxed_str());
    let interval = Arc::new(Mutex::new(3));
    // pass info to traffic tasks sent by the api server
    let (tx, mut rx) = mpsc::channel(32);
    // store connection handles
    let mut connections = JoinSet::new();
    let mut abort_handles = Vec::<tokio::task::AbortHandle>::new();
    // identify each connection
    let mut conn_id = 0;

    // spawn the api server
    tokio::spawn(start_api_server(tx.clone()));

    // blocks the main thread
    while let Some(params) = rx.recv().await {
        let n = params.connections.expect("connections param should exist");
        let tps = params.tps.expect("tps param should exist");
        println!("received: connections: {}, tps: {}", n, tps);

        update_interval(&interval, tps);

        if n == conn_id {
            continue;
        } else if n > conn_id {
            let new = n - conn_id;
            for _ in 0..new {
                conn_id += 1;
                let interval = interval.clone();
                let ah = connections.spawn(async move {
                    let mut conn = Connection::new(conn_id, interval, &url);
                    conn.wait_workers().await;
                });
                abort_handles.push(ah);
                println!(
                    "abort handles len after adding new: {}",
                    abort_handles.len()
                )
            }
        } else {
            // dropping connections
            let drop_n = conn_id - n;
            for _ in 0..drop_n {
                conn_id -= 1;
                if let Some(ah) = abort_handles.pop() {
                    ah.abort();
                    println!("abort handles len after abort: {}", abort_handles.len())
                }
            }
        }
    }
}
