use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time;
use tracing::{debug, error, info};

pub mod metrics;

#[derive(Clone)]
pub struct ConnectionClient {
    client: reqwest::Client,
}

impl ConnectionClient {
    fn builder() -> ConnectionClientBuilder {
        ConnectionClientBuilder {
            inner: reqwest::ClientBuilder::new()
                // always http2, always rust_tls for now
                .http2_prior_knowledge()
                .use_rustls_tls(),
        }
    }
}

struct ConnectionClientBuilder {
    inner: reqwest::ClientBuilder,
}

impl ConnectionClientBuilder {
    fn build(self) -> ConnectionClient {
        ConnectionClient {
            client: self.inner.build().unwrap(),
        }
    }

    fn identity(mut self, identity: reqwest::Identity) -> Self {
        self.inner = self
            .inner
            .identity(identity)
            .danger_accept_invalid_certs(true)
            .danger_accept_invalid_hostnames(true);
        self
    }
}

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
        pem: Option<String>,
    ) -> Connection {
        let mut builder = ConnectionClient::builder();
        if let Some(pem) = pem {
            let identity = match reqwest::Identity::from_pem(pem.as_bytes()) {
                Ok(id) => id,
                Err(e) => {
                    error!("Failed to parse pem file: {e}");
                    panic!();
                }
            };
            builder = builder.identity(identity);
        };
        let client = builder.build();
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
    fn new(
        id: String,
        url: &'static str,
        connection: ConnectionClient,
        mut wrx: watch::Receiver<u64>,
    ) -> Worker {
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

                        let resp = connection.client.get(url).send().await.unwrap();
                        metrics::inc_requests_sent();

                        // TODO: measure the time(latency) between these two
                        //
                        let body_fut = resp.bytes();

                        match time::timeout(time::Duration::from_millis(1000), body_fut).await {

                            Ok(Ok(_resp_bytes)) => {
                                metrics::inc_responses_received();
                            }
                            Ok(Err(e)) => {
                                error!("response received within the timeout but it is an Error: {}", e);

                                if e.to_string().contains("cannot decrypt peer's message") {
                                    metrics::inc_tls_failures();
                                    info!("ignoring tls failure, try another request");
                                    continue;
                                }

                            }
                            Err(e) => {
                                // timeout reached -- dropping resp
                                // since the client did not receive the response, implicitly it
                                // sends RST_STREAM
                                metrics::inc_stream_timeouts();
                                error!("stream timeout occured: {}", e);
                            }

                        }
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
