use axum::extract::Query;
use axum::routing::get;
use axum::Router;
use reqwest::{Client, ClientBuilder, Version};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::sync::Notify;
use tokio::task::JoinSet;
use tokio::time;

const URL: &str = "https://google.com";

struct Connection {
    client: Client,
    //tps: u16,
    connection_id: usize,
}

impl Connection {
    fn new(id: usize) -> Connection {
        Self {
            client: ClientBuilder::new()
                .http2_prior_knowledge()
                .build()
                .unwrap(),
            connection_id: id,
        }
    }

    async fn send(&self, notify: Arc<Notify>) {
        let mut interval = time::interval(time::Duration::from_millis(1000));

        println!("sending HTTP/2.0 from connection: {}", self.connection_id);
        loop {
            tokio::select! {
                _ = notify.notified() => {
                    println!("notification received -- dropping connection");
                    break;
                }
                _ = async {
                    interval.tick().await;
                    let resp = self.client.get(URL).send().await.unwrap();
                    assert_eq!(resp.version(), Version::HTTP_2);
                } => {}
            }
        }
    }
}

async fn root_handler(Query(params): Query<HashMap<String, String>>, tx: mpsc::Sender<String>) {
    for (k, v) in params {
        println!("key: {}, value: {}", k, v);
        tx.send(v).await.unwrap();
    }
}

async fn start_api_server(tx: mpsc::Sender<String>) {
    // build our application with a route
    let app = Router::new()
        // `GET /` goes to `root`
        .route("/", get(move |q| root_handler(q, tx)));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

#[tokio::main]
async fn main() {
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
    while let Some(n) = rx.recv().await {
        let n = n.parse::<usize>().expect("can't parse to usize");
        println!("n: {}", n);

        if n == conn_id {
            continue;
        } else if n > conn_id {
            // creating new connections
            let new = n - conn_id;
            for _ in 0..new {
                conn_id += 1;
                // pass one on each task
                let notify = notify.clone();
                set.spawn(async move {
                    let connection = Connection::new(conn_id);
                    connection.send(notify).await;
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
