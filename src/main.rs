use reqwest::{Client, ClientBuilder, Version};
use tokio::time;

const URL: &str = "https://google.com";

struct Connection {
    client: Client,
    //tps: u16,
    connection_id: u16,
}

impl Connection {
    fn new(id: u16) -> Connection {
        Self {
            client: ClientBuilder::new()
                .http2_prior_knowledge()
                .build()
                .unwrap(),
            connection_id: id,
        }
    }

    async fn send(&self) {
        let mut interval = time::interval(time::Duration::from_millis(1000));

        println!("sending HTTP/2.0 from connection: {}", self.connection_id);
        loop {
            interval.tick().await;
            let resp = self.client.get(URL).send().await.unwrap();

            assert_eq!(resp.version(), Version::HTTP_2);
        }
    }
}

#[tokio::main]
async fn main() {
    let mut tasks = Vec::with_capacity(5);

    for i in 0..5 {
        let handle = tokio::spawn(async move {
            let connection = Connection::new(i);
            connection.send().await;
        });
        tasks.push(handle);
    }

    let mut outputs = Vec::with_capacity(tasks.len());
    for task in tasks {
        outputs.push(task.await.unwrap());
    }
}
