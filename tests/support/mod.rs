#![allow(dead_code)]

use std::{
    collections::{BTreeMap, VecDeque},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::JoinHandle,
};

#[derive(Clone)]
pub struct CapturedRequest {
    pub method: String,
    pub target: String,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

impl CapturedRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
    }

    pub fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

#[derive(Clone)]
pub enum Script {
    Respond(ResponseScript),
    Disconnect,
    Hang(Duration),
}

#[derive(Clone)]
pub struct ResponseScript {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub headers_delay: Duration,
    pub body_delay: Duration,
    pub omit_content_length: bool,
    pub declared_content_length: Option<usize>,
}

impl ResponseScript {
    pub fn json(status: u16, value: serde_json::Value) -> Self {
        Self {
            status,
            headers: vec![("content-type".into(), "application/json".into())],
            body: serde_json::to_vec(&value).unwrap(),
            headers_delay: Duration::ZERO,
            body_delay: Duration::ZERO,
            omit_content_length: false,
            declared_content_length: None,
        }
    }

    pub fn bytes(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: body.into(),
            headers_delay: Duration::ZERO,
            body_delay: Duration::ZERO,
            omit_content_length: false,
            declared_content_length: None,
        }
    }

    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    pub fn headers_delay(mut self, value: Duration) -> Self {
        self.headers_delay = value;
        self
    }

    pub fn body_delay(mut self, value: Duration) -> Self {
        self.body_delay = value;
        self
    }

    pub fn without_content_length(mut self) -> Self {
        self.omit_content_length = true;
        self
    }

    pub fn declared_content_length(mut self, value: usize) -> Self {
        self.declared_content_length = Some(value);
        self
    }
}

pub struct ScriptedServer {
    base_url: String,
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
    max_in_flight: Arc<AtomicUsize>,
    task: JoinHandle<()>,
}

impl ScriptedServer {
    pub async fn start(scripts: impl IntoIterator<Item = Script>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let scripts = Arc::new(Mutex::new(scripts.into_iter().collect::<VecDeque<_>>()));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let max_in_flight = Arc::new(AtomicUsize::new(0));
        let active = Arc::new(AtomicUsize::new(0));

        let task_scripts = scripts.clone();
        let task_requests = requests.clone();
        let task_max = max_in_flight.clone();
        let task_active = active.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let script = task_scripts.lock().unwrap().pop_front().unwrap_or_else(|| {
                    Script::Respond(ResponseScript::json(
                        500,
                        serde_json::json!({"success": 0, "message": "missing script"}),
                    ))
                });
                let requests = task_requests.clone();
                let max_in_flight = task_max.clone();
                let active = task_active.clone();
                tokio::spawn(async move {
                    handle_connection(stream, script, requests, active, max_in_flight).await;
                });
            }
        });

        Self {
            base_url: format!("http://{address}/"),
            requests,
            max_in_flight,
            task,
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn requests(&self) -> Vec<CapturedRequest> {
        self.requests.lock().unwrap().clone()
    }

    pub fn request_count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }

    pub fn max_in_flight(&self) -> usize {
        self.max_in_flight.load(Ordering::SeqCst)
    }

    pub async fn wait_for_requests(&self, count: usize) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while self.request_count() < count {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("scripted server did not receive the expected requests");
    }
}

impl Drop for ScriptedServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn handle_connection(
    mut stream: TcpStream,
    script: Script,
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
    active: Arc<AtomicUsize>,
    max_in_flight: Arc<AtomicUsize>,
) {
    let Some(request) = read_request(&mut stream).await else {
        return;
    };
    requests.lock().unwrap().push(request);
    let current = active.fetch_add(1, Ordering::SeqCst) + 1;
    max_in_flight.fetch_max(current, Ordering::SeqCst);

    match script {
        Script::Disconnect => {}
        Script::Hang(duration) => tokio::time::sleep(duration).await,
        Script::Respond(response) => {
            tokio::time::sleep(response.headers_delay).await;
            let declared_length = response
                .declared_content_length
                .or((!response.omit_content_length).then_some(response.body.len()));
            let mut head = format!(
                "HTTP/1.1 {} {}\r\nconnection: close\r\n",
                response.status,
                reason_phrase(response.status)
            );
            if let Some(length) = declared_length {
                head.push_str(&format!("content-length: {length}\r\n"));
            }
            for (name, value) in response.headers {
                head.push_str(&format!("{name}: {value}\r\n"));
            }
            head.push_str("\r\n");
            if stream.write_all(head.as_bytes()).await.is_ok() {
                let _ = stream.flush().await;
                tokio::time::sleep(response.body_delay).await;
                let _ = stream.write_all(&response.body).await;
                let _ = stream.shutdown().await;
            }
        }
    }

    active.fetch_sub(1, Ordering::SeqCst);
}

async fn read_request(stream: &mut TcpStream) -> Option<CapturedRequest> {
    const MAX_REQUEST: usize = 2 * 1024 * 1024;
    let mut data = Vec::new();
    let mut scratch = [0_u8; 8192];
    let header_end = loop {
        let read = stream.read(&mut scratch).await.ok()?;
        if read == 0 {
            return None;
        }
        data.extend_from_slice(&scratch[..read]);
        if data.len() > MAX_REQUEST {
            return None;
        }
        if let Some(offset) = find_bytes(&data, b"\r\n\r\n") {
            break offset + 4;
        }
    };

    let head = String::from_utf8_lossy(&data[..header_end]);
    let mut lines = head.split("\r\n");
    let mut request_line = lines.next()?.split_whitespace();
    let method = request_line.next()?.to_owned();
    let target = request_line.next()?.to_owned();
    let mut headers = BTreeMap::new();
    for line in lines.filter(|line| !line.is_empty()) {
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
        }
    }

    let mut body = data[header_end..].to_vec();
    if let Some(length) = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
    {
        while body.len() < length {
            let read = stream.read(&mut scratch).await.ok()?;
            if read == 0 {
                break;
            }
            body.extend_from_slice(&scratch[..read]);
            if body.len() > MAX_REQUEST {
                return None;
            }
        }
        body.truncate(length);
    } else if headers
        .get("transfer-encoding")
        .is_some_and(|value| value.to_ascii_lowercase().contains("chunked"))
    {
        while !body.ends_with(b"0\r\n\r\n") {
            let read = stream.read(&mut scratch).await.ok()?;
            if read == 0 {
                break;
            }
            body.extend_from_slice(&scratch[..read]);
        }
        body = decode_chunked(&body)?;
    }

    Some(CapturedRequest {
        method,
        target,
        headers,
        body,
    })
}

fn decode_chunked(encoded: &[u8]) -> Option<Vec<u8>> {
    let mut cursor = 0;
    let mut decoded = Vec::new();
    loop {
        let line_end = find_bytes(&encoded[cursor..], b"\r\n")? + cursor;
        let size_text = std::str::from_utf8(&encoded[cursor..line_end]).ok()?;
        let size = usize::from_str_radix(size_text.split(';').next()?.trim(), 16).ok()?;
        cursor = line_end + 2;
        if size == 0 {
            return Some(decoded);
        }
        let end = cursor.checked_add(size)?;
        decoded.extend_from_slice(encoded.get(cursor..end)?);
        cursor = end.checked_add(2)?;
    }
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "Response",
    }
}
