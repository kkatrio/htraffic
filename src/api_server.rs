use crate::Params;
use axum::extract::Query;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use tokio::sync::mpsc;
use tracing::{error, info, span, Level};

async fn root_handler(Query(params): Query<Params>, tx: mpsc::Sender<Params>) -> impl IntoResponse {
    if params.connections.is_some() && params.tps.is_some() && params.workers.is_some() {
        // TODO: log what we received here
        if let Err(e) = tx.send(params.clone()).await {
            error!("failed to send params with: {e}");
        }
        format!("got it.\n")
    } else {
        "Missing required query parameters: 'connections' and 'tps'".into()
    }
}

pub async fn start_api_server(tx: mpsc::Sender<Params>) {
    // is the same span always be used in the root_handler?
    let span = span!(Level::INFO, "api_server");
    let _guard = span.enter();
    let app = Router::new()
        // `GET /` goes to `root`
        .route("/", get(move |q| root_handler(q, tx)));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    info!("started api server on 0.0.0.0:3000");
    axum::serve(listener, app).await.unwrap();
}
