use std::error::Error;

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
                .use_rustls_tls()
                .pool_max_idle_per_host(0),
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
        interval_receiver: watch::Receiver<u64>,
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
            let interval_receiver = interval_receiver.clone();
            let worker_id = format!("{}-{}", id.to_string(), i.to_string());
            workers.push(Worker::new(worker_id, url, client, interval_receiver))
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
        _id: String,
        url: &'static str,
        connection: ConnectionClient,
        mut interval_receiver: watch::Receiver<u64>,
    ) -> Worker {
        let task = tokio::spawn(async move {
            let mut intv = *interval_receiver.borrow();
            let mut interval = time::interval(time::Duration::from_micros(intv));

            loop {
                tokio::select! {
                    _ = interval_receiver.changed() => {
                        intv = *interval_receiver.borrow();
                        interval = time::interval(time::Duration::from_micros(intv));
                        debug!("interval changed, duration: {} ms", interval.period().as_millis());
                    }
                    _ = interval.tick() => {

                        let resp = connection.client.get(url).send().await;

                        let resp = match resp {
                            Ok(resp) => {
                                metrics::inc_requests_sent();
                                resp
                            }
                            Err(e) => {
                                if let Some(hyper_err) = e.source().and_then(|source| source.downcast_ref::<hyper::Error>()) {
                                    if let Some(h2_err) = hyper_err.source().and_then(|source| source.downcast_ref::<h2::Error>()) {
                                        match h2_err.reason() {
                                            Some(h2::Reason::REFUSED_STREAM) => {
                                                error!("got REFUSED_STREAM, ignoring");
                                                metrics::inc_refused_streams();
                                            }
                                            Some(reason) => {
                                                error!("got reason: {}, ignoring", reason.description());
                                            }
                                            None => {
                                                panic!("got h2 error, but without any known reason!");
                                            }
                                        }
                                    }
                                    error!("could downcast to hyper error, but not to h2. error. source {:?}", hyper_err.source());
                                    // we move to the next resp, no need to print this line again
                                    // below
                                    continue;
                                }
                                metrics::inc_unable_to_send();
                                error!("error received -- not downcast to hyper error. source: {:?}", e.source());
                                // in any error (downcast or not) we continut with the next resp, so we do not care about
                                // returning the same as in the Ok branch
                                continue;
                            }
                        };

                        // wait until we read the whole response
                        let body_fut = resp.bytes();

                        match time::timeout(time::Duration::from_millis(1000), body_fut).await {

                            Ok(Ok(_resp_bytes)) => {
                                metrics::inc_responses_received();
                            }
                            Ok(Err(e)) => {
                                error!("response received within the timeout but it is an Error: {}", e);

                                // TODO: move this above
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
