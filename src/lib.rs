use reqwest::{Client, ClientBuilder};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time;
use tracing::{debug, info};

pub struct Connection {
    connection_id: usize,
    workers: Vec<Worker>,
}

impl Connection {
    // should watch_rx be a reference here?
    pub fn new(
        id: usize,
        url: &'static str,
        watch_rx: watch::Receiver<u64>,
        number_of_workers: usize,
        pem: String,
    ) -> Connection {
        let identity = reqwest::Identity::from_pem(pem.as_bytes()).unwrap();

        let client = ClientBuilder::new()
            .http2_prior_knowledge()
            .use_rustls_tls()
            .identity(identity)
            .danger_accept_invalid_certs(true)
            .danger_accept_invalid_hostnames(true)
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

    pub async fn wait_workers(&mut self) {
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
                        // TODO: measure the time(latency) between these two
                        let _body = resp.bytes().await.unwrap();
                        // TODO: wait fo a second, then send a RESET and kill this worker
                        // TODO: step counter here
                    }
                }
            }
        });

        Worker { task }
    }
}

// not sure if this is done anyway
impl Drop for Worker {
    fn drop(&mut self) {
        info!("dropping worker...");
        self.task.abort();
    }
}
