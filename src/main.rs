use axum::extract::Query;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use clap::Parser;
use reqwest::{Client, ClientBuilder};
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::sync::Notify;
use tokio::task::JoinSet;
use tokio::time;

#[derive(Parser, Debug)]
#[command(author, version, about)]
pub struct Config {
    /// The URL to send HTTP/2 traffic to
    #[arg(short, long)]
    pub path: String,
}

struct Connection {
    client: Client,
    //tps: u16,
    connection_id: usize,
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct Params {
    pub connections: Option<usize>,
    pub tps: Option<u64>,
}

impl Connection {
    fn new(id: usize) -> Connection {
        Self {
            client: ClientBuilder::new()
                .http2_prior_knowledge()
                .use_rustls_tls()
                .build()
                .unwrap(),
            connection_id: id,
        }
    }

    async fn send(&self, notify: Arc<Notify>, interval: u64, url: &str) {
        let mut interval = time::interval(time::Duration::from_micros(interval));

        println!("sending HTTP/2.0 from connection: {}", self.connection_id);
        loop {
            tokio::select! {
                _ = notify.notified() => {
                    println!("notification received -- dropping connection");
                    break;
                }
                _ = async {
                    interval.tick().await;
                    let _ = self.client.get(url).send().await.unwrap();
                    //assert_eq!(resp.version(), Version::HTTP_2);
                } => {}
            }
        }
    }
}

async fn root_handler(Query(params): Query<Params>, tx: mpsc::Sender<Params>) -> impl IntoResponse {
    if params.connections.is_some() && params.tps.is_some() {
        if let Err(e) = tx.send(params.clone()).await {
            eprintln!("failed to send params with: {e}");
        }
        format!("Sent: {params:?}")
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
    axum::serve(listener, app).await.unwrap();
}

#[tokio::main]
async fn main() {
    let config = Config::parse();
    let url = Arc::new(config.path);

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

        // todo: handle this some other way
        let interval = if tps != 0 { 1000000 / tps } else { u64::MAX };

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
                set.spawn(async move {
                    let connection = Connection::new(conn_id);
                    connection.send(notify, interval, &url).await;
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
