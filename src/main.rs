use axum::extract::Query;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use clap::Parser;
use reqwest::{Client, ClientBuilder};
use serde::Deserialize;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::mpsc;
use tokio::sync::Notify;
use tokio::task::JoinSet;
use tokio::time;
use tracing_subscriber;

#[derive(Parser, Debug)]
#[command(author, version, about)]
pub struct Config {
    /// The URL to send HTTP/2 traffic to
    #[arg(short, long)]
    pub path: String,
}

struct Connection {
    client: Client,
    connection_id: usize,
    interval: Arc<Mutex<u64>>,
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct Params {
    pub connections: Option<usize>,
    pub tps: Option<u64>,
}

impl Connection {
    fn new(id: usize, interval: Arc<Mutex<u64>>) -> Connection {
        Self {
            client: ClientBuilder::new()
                .http2_prior_knowledge()
                .use_rustls_tls()
                .build()
                .unwrap(),
            connection_id: id,
            interval: interval,
        }
    }

    async fn send(&self, notify: Arc<Notify>, url: &str) {
        println!("sending HTTP/2.0 from connection: {}", self.connection_id);

        let mut intv = {
            let guard = self.interval.lock().unwrap();
            *guard
        };
        let mut interval = time::interval(time::Duration::from_micros(intv));

        loop {
            let new_intv = {
                let guard = self.interval.lock().unwrap();
                *guard
            };

            if new_intv != intv {
                //println!("changing intv");
                intv = new_intv;
                interval = time::interval(time::Duration::from_micros(intv));
            }

            tokio::select! {
                _ = notify.notified() => {
                    println!("notification received -- dropping connection");
                    break;
                }
                _ = interval.tick() => {
                    //println!("period: {}, tick at: {:?}",  interval.period().as_micros(), Instant::now());
                    let resp = self.client.get(url).send().await.unwrap();
                    let _body = resp.bytes().await.unwrap();
                }
            }
        }
    }
}

async fn root_handler(Query(params): Query<Params>, tx: mpsc::Sender<Params>) -> impl IntoResponse {
    if params.connections.is_some() && params.tps.is_some() {
        if let Err(e) = tx.send(params.clone()).await {
            eprintln!("failed to send params with: {e}");
        }
        format!("got it.")
    } else {
        "Missing required query parameters: 'connections' and 'tps'".into()
    }
}

async fn start_api_server(tx: mpsc::Sender<Params>) {
    // build our application with a route
    let app = Router::new()
        // `GET /` goes to `root`
        .route("/", get(move |q| root_handler(q, tx)));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    println!("started api server on 0.0.0.0:3000");
    axum::serve(listener, app).await.unwrap();
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    let config = Config::parse();
    let url = Arc::new(config.path);
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
        println!("received connections: {}, tps: {}", n, tps);

        {
            let mut iguard = interval.lock().unwrap();
            *iguard = if tps != 0 { 1000000 / tps } else { u64::MAX };
        };

        if n == conn_id {
            continue;
        } else if n > conn_id {
            // creating new connections
            let new = n - conn_id;
            for _ in 0..new {
                conn_id += 1;
                // pass one on each task
                let notify = notify.clone();
                let url = url.clone();
                let interval = interval.clone();
                set.spawn(async move {
                    let connection = Connection::new(conn_id, interval);
                    connection.send(notify, &url).await;
                });
            }
        } else {
            // dropping connections
            let drop_n = conn_id - n;
            for _ in 0..drop_n {
                println!("sending notify_one");
                notify.notify_one();
                conn_id -= 1;
            }
        }
    }

    // not necessary, since the receiver blocks
    //set.join_all().await;
}
