use clap::Parser;
use reqwest::{Client, ClientBuilder};
use serde::Deserialize;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tokio::sync::Notify;
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
    id: usize,
    pub task: JoinHandle<()>,
}

impl Worker {
    fn new(id: usize, url: &'static str, client: Client, interval: Arc<Mutex<u64>>) -> Worker {
        let intv = {
            let guard = interval.lock().unwrap();
            *guard
        };
        println!("worker id: {}, ntv: {}", id, intv);

        let mut interval = time::interval(time::Duration::from_micros(intv));

        let task = tokio::spawn(async move {
            loop {
                interval.tick().await;
                println!("sending from client with id {}", id);
                let resp = client.get(url).send().await.unwrap();
                let _body = resp.bytes().await.unwrap();
            }
        });

        Worker { id, task }
    }
}

impl Connection {
    fn new(id: usize, interval: Arc<Mutex<u64>>, url: &'static str) -> Connection {
        // internally!
        let client = ClientBuilder::new()
            .http2_prior_knowledge()
            .use_rustls_tls()
            .connection_verbose(true)
            .build()
            .unwrap();
        //let client = Arc::new(client);

        let mut worker_threads = Vec::with_capacity(2);
        for id in 0..2 {
            let client = client.clone();
            let interval = interval.clone();
            worker_threads.push(Worker::new(id, url, client, interval))
        }

        Self {
            connection_id: id,
            interval: interval,
            workers: worker_threads,
        }
    }

    async fn wait_workers(&mut self) {
        println!("awaiting workers");
        let mut outputs = Vec::with_capacity(self.workers.len());
        for worker in &mut self.workers {
            let task = &mut worker.task;
            outputs.push(task.await.unwrap());
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
    //let url = Arc::new(config.path);

    let url: &'static str = Box::leak(config.path.into_boxed_str());

    let interval = Arc::new(Mutex::new(3));

    // pass info to traffic tasks sent by the api server
    let (tx, mut rx) = mpsc::channel(32);
    // store connection handles
    let mut set = JoinSet::new();
    // identify each connection
    let mut conn_id = 0;
    // notify client loop to break when dropping connections
    let notify = Arc::new(Notify::new());

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
            // creating new connections
            let new = n - conn_id;
            for _ in 0..new {
                conn_id += 1;
                // pass one on each task
                let _notify = notify.clone();
                let interval = interval.clone();
                set.spawn(async move {
                    let mut connection = Connection::new(conn_id, interval, &url);
                    connection.wait_workers().await;
                    //connection.send(notify).await;
                });
            }
        } else {
            // dropping connections
            let drop_n = conn_id - n;
            for _ in 0..drop_n {
                //println!("sending notify_one");
                notify.notify_one();
                conn_id -= 1;
            }
        }
    }

    // not necessary, since the receiver blocks
    //set.join_all().await;
}
