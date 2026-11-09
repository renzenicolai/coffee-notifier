use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use crate::config::NotifierConfig;
use crate::event::Event;
use crate::notifier::NotifierPublish;
use crate::state::{OngoingSnapshot, SharedState};

/// The daemon holding the real MQTT connection is the only process that can
/// publish. The dialog process is short-lived and talks to it over this
/// loopback-only, newline-delimited JSON protocol.
#[derive(Debug, Serialize, Deserialize)]
struct Envelope {
    token: String,
    request: Request,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
enum Request {
    GetOngoing,
    Publish { event: Event },
    PublishNotifier { publish: NotifierPublish },
    GetConfig,
    SetConfig { config: NotifierConfig },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum Response {
    Ongoing { events: Vec<OngoingSnapshot> },
    Config { config: NotifierConfig },
    Ack,
    Error { message: String },
}

pub fn serve(
    listener: TcpListener,
    token: String,
    shared: Arc<SharedState>,
    outbox: tokio::sync::mpsc::UnboundedSender<Event>,
    outbox_notifier: tokio::sync::mpsc::UnboundedSender<NotifierPublish>,
    config_tx: watch::Sender<NotifierConfig>,
) {
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let shared = shared.clone();
            let outbox = outbox.clone();
            let outbox_notifier = outbox_notifier.clone();
            let config_tx = config_tx.clone();
            let token = token.clone();
            std::thread::spawn(move || {
                handle_client(stream, &token, &shared, &outbox, &outbox_notifier, &config_tx)
            });
        }
    });
}

fn handle_client(
    stream: TcpStream,
    token: &str,
    shared: &Arc<SharedState>,
    outbox: &tokio::sync::mpsc::UnboundedSender<Event>,
    outbox_notifier: &tokio::sync::mpsc::UnboundedSender<NotifierPublish>,
    config_tx: &watch::Sender<NotifierConfig>,
) {
    let Ok(mut writer) = stream.try_clone() else {
        return;
    };
    let reader = BufReader::new(stream);

    for line in reader.lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }

        let response = match serde_json::from_str::<Envelope>(&line) {
            Ok(envelope) if envelope.token != token => Response::Error {
                message: "invalid token".to_string(),
            },
            Ok(envelope) => match envelope.request {
                Request::GetOngoing => Response::Ongoing {
                    events: shared.snapshot(),
                },
                Request::Publish { event } => match outbox.send(event) {
                    Ok(()) => Response::Ack,
                    Err(_) => Response::Error {
                        message: "daemon is no longer accepting events".to_string(),
                    },
                },
                Request::PublishNotifier { publish } => match outbox_notifier.send(publish) {
                    Ok(()) => Response::Ack,
                    Err(_) => Response::Error {
                        message: "daemon is no longer accepting events".to_string(),
                    },
                },
                Request::GetConfig => Response::Config {
                    config: shared.config.lock().unwrap().clone(),
                },
                Request::SetConfig { config } => match config.save() {
                    Ok(()) => {
                        *shared.config.lock().unwrap() = config.clone();
                        let _ = config_tx.send(config);
                        Response::Ack
                    }
                    Err(e) => Response::Error {
                        message: format!("failed to save settings: {e}"),
                    },
                },
            },
            Err(e) => Response::Error {
                message: format!("bad request: {e}"),
            },
        };

        let Ok(json) = serde_json::to_string(&response) else {
            break;
        };
        if writeln!(writer, "{json}").is_err() {
            break;
        }
    }
}

pub struct IpcClient {
    token: String,
    reader: BufReader<TcpStream>,
    writer: TcpStream,
}

impl IpcClient {
    pub fn connect(port: u16, token: String) -> anyhow::Result<Self> {
        let writer = TcpStream::connect(("127.0.0.1", port))?;
        let reader = BufReader::new(writer.try_clone()?);
        Ok(Self {
            token,
            reader,
            writer,
        })
    }

    fn roundtrip(&mut self, request: Request) -> anyhow::Result<Response> {
        let envelope = Envelope {
            token: self.token.clone(),
            request,
        };
        let json = serde_json::to_string(&envelope)?;
        writeln!(self.writer, "{json}")?;

        let mut line = String::new();
        self.reader.read_line(&mut line)?;
        if line.is_empty() {
            anyhow::bail!("dcn-notify daemon closed the connection");
        }
        Ok(serde_json::from_str(&line)?)
    }

    pub fn get_ongoing(&mut self) -> anyhow::Result<Vec<OngoingSnapshot>> {
        match self.roundtrip(Request::GetOngoing)? {
            Response::Ongoing { events } => Ok(events),
            Response::Error { message } => anyhow::bail!(message),
            _ => anyhow::bail!("unexpected response"),
        }
    }

    pub fn publish(&mut self, event: Event) -> anyhow::Result<()> {
        match self.roundtrip(Request::Publish { event })? {
            Response::Ack => Ok(()),
            Response::Error { message } => anyhow::bail!(message),
            _ => anyhow::bail!("unexpected response"),
        }
    }

    pub fn publish_notifier(&mut self, publish: NotifierPublish) -> anyhow::Result<()> {
        match self.roundtrip(Request::PublishNotifier { publish })? {
            Response::Ack => Ok(()),
            Response::Error { message } => anyhow::bail!(message),
            _ => anyhow::bail!("unexpected response"),
        }
    }

    pub fn get_config(&mut self) -> anyhow::Result<NotifierConfig> {
        match self.roundtrip(Request::GetConfig)? {
            Response::Config { config } => Ok(config),
            Response::Error { message } => anyhow::bail!(message),
            _ => anyhow::bail!("unexpected response"),
        }
    }

    pub fn set_config(&mut self, config: NotifierConfig) -> anyhow::Result<()> {
        match self.roundtrip(Request::SetConfig { config })? {
            Response::Ack => Ok(()),
            Response::Error { message } => anyhow::bail!(message),
            _ => anyhow::bail!("unexpected response"),
        }
    }
}
