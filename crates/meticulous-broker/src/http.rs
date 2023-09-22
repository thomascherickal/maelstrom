use crate::{
    connection::{self, IdVendor},
    proto, Result, SchedulerMessage,
};
use futures::stream::{SplitSink, SplitStream};
use futures::{sink::SinkExt, stream::StreamExt};
use hyper::service::Service;
use hyper::upgrade::Upgraded;
use hyper::{Body, Request, Response};
use hyper_tungstenite::WebSocketStream;
use hyper_tungstenite::{tungstenite, HyperWebsocket};
use meticulous_base::{proto::BrokerToClient, ClientId};
use std::collections::HashMap;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tungstenite::Message;

const WASM_TAR: &[u8] = include_bytes!("../../../target/web.tar");

#[derive(Clone)]
pub struct TarHandler {
    map: HashMap<String, &'static [u8]>,
}

impl TarHandler {
    pub fn from_memory(bytes: &'static [u8]) -> Self {
        let mut map = HashMap::new();
        let mut ar = tar::Archive::new(bytes);
        for entry in ar.entries().unwrap() {
            let entry = entry.unwrap();
            let header = entry.header();

            let path = header.path().unwrap().to_str().unwrap().into();
            let start = entry.raw_file_position() as usize;
            let end = start + header.size().unwrap() as usize;
            map.insert(path, &bytes[start..end]);
        }
        Self { map }
    }

    fn get_file(&self, path: &str) -> Response<Body> {
        fn mime_for_path(path: &str) -> &'static str {
            if let Some(ext) = Path::new(path).extension() {
                match &ext.to_str().unwrap().to_lowercase()[..] {
                    "wasm" => return "application/wasm",
                    "js" => return "text/javascript",
                    "html" => return "text/html",
                    _ => (),
                }
            }
            "application/octet-stream"
        }

        let mut path = format!(".{}", path);

        if path == "./" {
            path = "./index.html".into();
        }

        self.map
            .get(&path[..])
            .map(|&b| {
                Response::builder()
                    .status(200)
                    .header("Content-Type", mime_for_path(&path))
                    .body(Body::from(b))
                    .unwrap()
            })
            .unwrap_or(
                Response::builder()
                    .status(404)
                    .body(Body::from(&b""[..]))
                    .unwrap(),
            )
    }
}

#[derive(Clone)]
struct Handler {
    tar_handler: TarHandler,
    scheduler_sender: UnboundedSender<SchedulerMessage>,
    id_vendor: Arc<IdVendor>,
}

impl Service<Request<Body>> for Handler {
    type Response = Response<Body>;
    type Error = crate::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut request: Request<Body>) -> Self::Future {
        let resp = (|| {
            if hyper_tungstenite::is_upgrade_request(&request) {
                let (response, websocket) = hyper_tungstenite::upgrade(&mut request, None)?;
                let scheduler_sender = self.scheduler_sender.clone();
                let id_vendor = self.id_vendor.clone();
                tokio::spawn(async move {
                    serve_websocket(websocket, scheduler_sender, id_vendor)
                        .await
                        .ok()
                });
                Ok(response)
            } else {
                Ok(self.tar_handler.get_file(&request.uri().to_string()))
            }
        })();

        Box::pin(async { resp })
    }
}

async fn websocket_writer(
    mut scheduler_receiver: UnboundedReceiver<BrokerToClient>,
    mut socket: SplitSink<WebSocketStream<Upgraded>, Message>,
) -> Result<()> {
    while let Some(msg) = scheduler_receiver.recv().await {
        socket
            .send(Message::binary(bincode::serialize(&msg).unwrap()))
            .await?
    }
    Ok(())
}

async fn websocket_reader(
    mut socket: SplitStream<WebSocketStream<Upgraded>>,
    scheduler_sender: UnboundedSender<SchedulerMessage>,
    id: ClientId,
) -> Result<()> {
    while let Some(Ok(Message::Binary(msg))) = socket.next().await {
        let msg = bincode::deserialize(&msg)?;
        if scheduler_sender
            .send(SchedulerMessage::FromClient(
                id,
                proto::ClientToBroker::UiRequest(msg),
            ))
            .is_err()
        {
            return Ok(());
        }
    }

    Ok(())
}

async fn serve_websocket(
    websocket: HyperWebsocket,
    scheduler_sender: UnboundedSender<SchedulerMessage>,
    id_vendor: Arc<IdVendor>,
) -> Result<()> {
    let websocket = websocket.await?;

    let (write_stream, read_stream) = websocket.split();

    let id = id_vendor.vend();

    connection::socket_main(
        scheduler_sender,
        id,
        SchedulerMessage::ClientConnected,
        SchedulerMessage::ClientDisconnected,
        |scheduler_sender| websocket_reader(read_stream, scheduler_sender, id),
        |scheduler_receiver| websocket_writer(scheduler_receiver, write_stream),
    )
    .await;

    Ok(())
}

pub async fn main(
    listener: tokio::net::TcpListener,
    scheduler_sender: UnboundedSender<SchedulerMessage>,
    id_vendor: Arc<IdVendor>,
) -> Result<()> {
    let mut http = hyper::server::conn::Http::new();
    http.http1_only(true);
    http.http1_keep_alive(true);

    let tar_handler = TarHandler::from_memory(WASM_TAR);

    loop {
        let (stream, _) = listener.accept().await?;
        let tar_handler = tar_handler.clone();
        let scheduler_sender = scheduler_sender.clone();
        let id_vendor = id_vendor.clone();
        let connection = http
            .serve_connection(
                stream,
                Handler {
                    tar_handler,
                    scheduler_sender,
                    id_vendor,
                },
            )
            .with_upgrades();
        tokio::spawn(async move { connection.await.ok() });
    }
}
