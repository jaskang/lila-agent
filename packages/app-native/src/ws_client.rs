//! Pure Rust WebSocket transport with `lila.<token>` auth, reconnect backoff,
//! typed command RPC, and 16ms delta/state frame coalescing (zero QuickJS).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;

use crate::core_host::{PROTOCOL_VERSION, ensure_core};
use crate::protocol::{ConnectionState, Project, SessionState, SessionSummary, TranscriptItem};

const RETRY_DELAYS_MS: [u64; 5] = [300, 600, 1_200, 2_500, 5_000];
const BATCH_WINDOW_MS: u64 = 16;

type PendingMap = Arc<Mutex<HashMap<String, oneshot::Sender<Result<Value, String>>>>>;

struct OutgoingCommand {
    request_id: String,
    payload: String,
    reply: oneshot::Sender<Result<Value, String>>,
}

#[derive(Debug, Clone)]
pub enum PushFrame {
    Projects(Vec<Project>),
    Session(SessionSummary),
    SessionRemoved(String),
    Item {
        session_id: String,
        item: TranscriptItem,
    },
    Delta {
        session_id: String,
        item_id: String,
        channel: String,
        delta: String,
    },
    State {
        session_id: String,
        state: SessionState,
    },
    Reset {
        session_id: String,
    },
    Notify {
        session_id: Option<String>,
        level: String,
        message: String,
    },
    Error {
        code: String,
        message: String,
    },
}

#[derive(Debug, Clone)]
pub enum TransportEvent {
    Connection {
        state: ConnectionState,
        detail: Option<String>,
    },
    Ready {
        core_version: String,
    },
    Batch(Vec<PushFrame>),
}

#[derive(Default)]
struct FrameBatch {
    frames: Vec<PushFrame>,
    delta_index: HashMap<(String, String, String), usize>,
    state_index: HashMap<String, usize>,
}

impl FrameBatch {
    fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    fn push(&mut self, frame: PushFrame) {
        match frame {
            PushFrame::Delta {
                session_id,
                item_id,
                channel,
                delta,
            } => {
                let key = (session_id.clone(), item_id.clone(), channel.clone());
                if let Some(&idx) = self.delta_index.get(&key)
                    && let Some(PushFrame::Delta {
                        delta: existing, ..
                    }) = self.frames.get_mut(idx)
                {
                    existing.push_str(&delta);
                    return;
                }
                let idx = self.frames.len();
                self.delta_index.insert(key, idx);
                self.frames.push(PushFrame::Delta {
                    session_id,
                    item_id,
                    channel,
                    delta,
                });
            }
            PushFrame::State { session_id, state } => {
                if let Some(&idx) = self.state_index.get(&session_id) {
                    self.frames[idx] = PushFrame::State { session_id, state };
                    return;
                }
                let idx = self.frames.len();
                self.state_index.insert(session_id.clone(), idx);
                self.frames.push(PushFrame::State { session_id, state });
            }
            PushFrame::Item { session_id, item } => {
                let item_id = item.id().to_owned();
                self.delta_index
                    .retain(|(sid, iid, _), _| !(sid == &session_id && iid == &item_id));
                self.frames.push(PushFrame::Item { session_id, item });
            }
            other => self.frames.push(other),
        }
    }

    fn take(&mut self) -> Vec<PushFrame> {
        self.delta_index.clear();
        self.state_index.clear();
        std::mem::take(&mut self.frames)
    }
}

fn parse_push_frame(msg: Value) -> Option<PushFrame> {
    let kind = msg.get("type")?.as_str()?;
    match kind {
        "projects" => {
            let projects: Vec<Project> = serde_json::from_value(msg.get("projects")?.clone()).ok()?;
            Some(PushFrame::Projects(projects))
        }
        "session" => {
            let session: SessionSummary = serde_json::from_value(msg.get("session")?.clone()).ok()?;
            Some(PushFrame::Session(session))
        }
        "session.removed" => Some(PushFrame::SessionRemoved(
            msg.get("sessionId")?.as_str()?.to_owned(),
        )),
        "item" => {
            let session_id = msg.get("sessionId")?.as_str()?.to_owned();
            let item: TranscriptItem = serde_json::from_value(msg.get("item")?.clone()).ok()?;
            Some(PushFrame::Item { session_id, item })
        }
        "delta" => Some(PushFrame::Delta {
            session_id: msg.get("sessionId")?.as_str()?.to_owned(),
            item_id: msg.get("itemId")?.as_str()?.to_owned(),
            channel: msg.get("channel")?.as_str()?.to_owned(),
            delta: msg.get("delta")?.as_str()?.to_owned(),
        }),
        "state" => {
            let session_id = msg.get("sessionId")?.as_str()?.to_owned();
            let state: SessionState = serde_json::from_value(msg.get("state")?.clone()).ok()?;
            Some(PushFrame::State { session_id, state })
        }
        "reset" => Some(PushFrame::Reset {
            session_id: msg.get("sessionId")?.as_str()?.to_owned(),
        }),
        "notify" => Some(PushFrame::Notify {
            session_id: msg
                .get("sessionId")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            level: msg
                .get("level")
                .and_then(Value::as_str)
                .unwrap_or("info")
                .to_owned(),
            message: msg
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
        }),
        "error" => Some(PushFrame::Error {
            code: msg
                .get("code")
                .and_then(Value::as_str)
                .unwrap_or("error")
                .to_owned(),
            message: msg
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
        }),
        _ => None,
    }
}

fn io_runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("lila-ws-io")
            .build()
            .expect("failed to build Lila WS tokio runtime")
    })
}

fn fail_all_pending(pending: &PendingMap, reason: &str) {
    if let Ok(mut guard) = pending.lock() {
        for (_, tx) in guard.drain() {
            let _ = tx.send(Err(reason.to_owned()));
        }
    }
}

#[derive(Clone)]
pub struct CoreRpc {
    cmd_tx: mpsc::UnboundedSender<OutgoingCommand>,
    stopped: Arc<AtomicBool>,
    next_id: Arc<AtomicU64>,
}

impl CoreRpc {
    pub fn start() -> (Self, mpsc::UnboundedReceiver<TransportEvent>) {
        let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<OutgoingCommand>();
        let (evt_tx, evt_rx) = mpsc::unbounded_channel::<TransportEvent>();
        let stopped = Arc::new(AtomicBool::new(false));
        let bg_stopped = stopped.clone();

        io_runtime().spawn(async move {
            let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
            let mut attempt: usize = 0;

            while !bg_stopped.load(Ordering::SeqCst) {
                let _ = evt_tx.send(TransportEvent::Connection {
                    state: ConnectionState::Connecting,
                    detail: None,
                });

                let conn_info = match ensure_core().await {
                    Ok(info) => info,
                    Err(err) => {
                        eprintln!("[lila-native] ensure_core failed: {err}");
                        let _ = evt_tx.send(TransportEvent::Connection {
                            state: ConnectionState::Disconnected,
                            detail: Some(err),
                        });
                        let delay = RETRY_DELAYS_MS[attempt.min(RETRY_DELAYS_MS.len() - 1)];
                        attempt += 1;
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                        continue;
                    }
                };

                let mut request = match conn_info.ws_url.as_str().into_client_request() {
                    Ok(req) => req,
                    Err(err) => {
                        let _ = evt_tx.send(TransportEvent::Connection {
                            state: ConnectionState::Disconnected,
                            detail: Some(format!("无效 WebSocket 地址: {err}")),
                        });
                        tokio::time::sleep(Duration::from_secs(2)).await;
                        continue;
                    }
                };
                let subprotocol = format!("lila.{}", conn_info.token);
                if let Ok(header) = HeaderValue::from_str(&subprotocol) {
                    request
                        .headers_mut()
                        .insert("Sec-WebSocket-Protocol", header);
                }

                let ws_stream = match tokio_tungstenite::connect_async(request).await {
                    Ok((stream, _)) => stream,
                    Err(err) => {
                        eprintln!("[lila-native] ws connect failed ({}): {err}", conn_info.ws_url);
                        let _ = evt_tx.send(TransportEvent::Connection {
                            state: ConnectionState::Disconnected,
                            detail: Some(format!("连接 core 失败: {err}")),
                        });
                        let delay = RETRY_DELAYS_MS[attempt.min(RETRY_DELAYS_MS.len() - 1)];
                        attempt += 1;
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                        continue;
                    }
                };

                let (mut ws_write, mut ws_read) = ws_stream.split();
                let mut ready = false;
                let mut batch = FrameBatch::default();
                let flush_sleep = tokio::time::sleep(Duration::from_secs(3600));
                tokio::pin!(flush_sleep);
                let mut flush_armed = false;
                let mut incompatible = false;

                loop {
                    if bg_stopped.load(Ordering::SeqCst) {
                        let _ = ws_write.close().await;
                        break;
                    }

                    tokio::select! {
                        biased;

                        maybe_cmd = cmd_rx.recv() => {
                            let Some(cmd) = maybe_cmd else {
                                bg_stopped.store(true, Ordering::SeqCst);
                                let _ = ws_write.close().await;
                                break;
                            };
                            if !ready {
                                let _ = cmd.reply.send(Err("与 core 的连接尚未就绪，正在重连…".to_owned()));
                                continue;
                            }
                            if let Ok(mut guard) = pending.lock() {
                                guard.insert(cmd.request_id.clone(), cmd.reply);
                            }
                            if let Err(err) = ws_write.send(Message::Text(cmd.payload.into())).await {
                                if let Ok(mut guard) = pending.lock()
                                    && let Some(reply) = guard.remove(&cmd.request_id)
                                {
                                    let _ = reply.send(Err(format!("发送命令失败: {err}")));
                                }
                                break;
                            }
                        }

                        () = &mut flush_sleep, if flush_armed => {
                            flush_armed = false;
                            if !batch.is_empty() {
                                let _ = evt_tx.send(TransportEvent::Batch(batch.take()));
                            }
                        }

                        maybe_frame = ws_read.next() => {
                            match maybe_frame {
                                Some(Ok(Message::Text(text))) => {
                                    let Ok(msg) = serde_json::from_str::<Value>(text.as_str()) else {
                                        continue;
                                    };
                                    match msg.get("type").and_then(Value::as_str) {
                                        Some("hello") => {
                                            let pv = msg
                                                .get("protocolVersion")
                                                .and_then(Value::as_u64)
                                                .unwrap_or(0) as u32;
                                            let core_ver = msg
                                                .get("coreVersion")
                                                .and_then(Value::as_str)
                                                .unwrap_or("unknown")
                                                .to_owned();
                                            if pv != PROTOCOL_VERSION {
                                                incompatible = true;
                                                let _ = evt_tx.send(TransportEvent::Connection {
                                                    state: ConnectionState::Incompatible,
                                                    detail: Some(format!("core 协议版本 {pv}，界面需要 {PROTOCOL_VERSION}。")),
                                                });
                                                let _ = ws_write.close().await;
                                                break;
                                            }
                                            attempt = 0;
                                            ready = true;
                                            let _ = evt_tx.send(TransportEvent::Connection {
                                                state: ConnectionState::Connected,
                                                detail: None,
                                            });
                                            let _ = evt_tx.send(TransportEvent::Ready {
                                                core_version: core_ver,
                                            });
                                        }
                                        Some("response") => {
                                            if let Some(req_id) = msg.get("requestId").and_then(Value::as_str)
                                                && let Ok(mut guard) = pending.lock()
                                                && let Some(reply) = guard.remove(req_id)
                                            {
                                                if msg.get("ok").and_then(Value::as_bool) == Some(true) {
                                                    let data = msg.get("data").cloned().unwrap_or(json!({}));
                                                    let _ = reply.send(Ok(data));
                                                } else {
                                                    let err = msg
                                                        .get("error")
                                                        .and_then(Value::as_str)
                                                        .unwrap_or("命令执行失败")
                                                        .to_owned();
                                                    let _ = reply.send(Err(err));
                                                }
                                            }
                                        }
                                        Some("reset") => {
                                            if ready {
                                                if !batch.is_empty() {
                                                    let _ = evt_tx.send(TransportEvent::Batch(batch.take()));
                                                }
                                                flush_armed = false;
                                                if let Some(frame) = parse_push_frame(msg) {
                                                    let _ = evt_tx.send(TransportEvent::Batch(vec![frame]));
                                                }
                                            }
                                        }
                                        _ => {
                                            if ready && let Some(frame) = parse_push_frame(msg) {
                                                batch.push(frame);
                                                if !flush_armed {
                                                    flush_sleep.as_mut().reset(
                                                        tokio::time::Instant::now()
                                                            + Duration::from_millis(BATCH_WINDOW_MS),
                                                    );
                                                    flush_armed = true;
                                                }
                                            }
                                        }
                                    }
                                }
                                Some(Ok(Message::Ping(bytes))) => {
                                    let _ = ws_write.send(Message::Pong(bytes)).await;
                                }
                                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                                _ => {}
                            }
                        }
                    }
                }

                if !batch.is_empty() {
                    let _ = evt_tx.send(TransportEvent::Batch(batch.take()));
                }
                fail_all_pending(&pending, "与 core 的连接已断开");

                if incompatible || bg_stopped.load(Ordering::SeqCst) {
                    break;
                }

                let _ = evt_tx.send(TransportEvent::Connection {
                    state: ConnectionState::Disconnected,
                    detail: None,
                });
                let delay = RETRY_DELAYS_MS[attempt.min(RETRY_DELAYS_MS.len() - 1)];
                attempt += 1;
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
        });

        let rpc = Self {
            cmd_tx,
            stopped,
            next_id: Arc::new(AtomicU64::new(1)),
        };
        (rpc, evt_rx)
    }

    pub async fn call<T: DeserializeOwned>(&self, mut command: Value) -> Result<T, String> {
        let Some(obj) = command.as_object_mut() else {
            return Err("命令必须是 JSON 对象".to_owned());
        };
        let seq = self.next_id.fetch_add(1, Ordering::Relaxed);
        let request_id = format!("rust-{seq}");
        obj.insert("requestId".to_owned(), Value::String(request_id.clone()));
        let payload = serde_json::to_string(&command).map_err(|e| e.to_string())?;
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(OutgoingCommand {
                request_id,
                payload,
                reply: tx,
            })
            .map_err(|_| "与 core 的传输通道已关闭".to_owned())?;
        let data = rx
            .await
            .map_err(|_| "请求已取消".to_owned())??;
        serde_json::from_value::<T>(data).map_err(|e| format!("解析 core 响应失败: {e}"))
    }

    pub async fn call_unit(&self, command: Value) -> Result<(), String> {
        let _: Value = self.call(command).await?;
        Ok(())
    }

    pub fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
    }
}
