use serde::Deserialize;
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::mpsc::{self, Receiver},
    thread,
};
use tracing::{info, warn};

const CALLBACK_ADDR: &str = "127.0.0.1:38291";
const PURCHASE_CALLBACK_ADDR: &str = "127.0.0.1:38292";
const VGLD_CALLBACK_ADDR: &str = "127.0.0.1:38293";
const BRIDGE_HTML: &str = include_str!("../../wallet-bridge/index.html");
const PURCHASE_HTML: &str = include_str!("../../wallet-bridge/purchase.html");
const VGLD_HTML: &str = include_str!("../../wallet-bridge/vgld.html");
const CHALLENGE_PLACEHOLDER: &str = "__VELOREN_CHALLENGE__";

#[derive(Debug, Deserialize)]
pub struct WalletLink {
    pub wallet: String,
    pub challenge: String,
    pub signature: String,
}

#[derive(Debug, Deserialize)]
pub struct PropertyPurchase {
    pub wallet: String,
    pub parcel_id: String,
    pub signature: String,
}

#[derive(Debug, Deserialize)]
pub struct VgldDeposit {
    pub wallet: String,
    pub amount: u64,
    pub signature: String,
}

pub struct WalletBridge {
    receiver: Receiver<WalletLink>,
}

impl WalletBridge {
    pub fn start(challenge: &str) -> Result<Self, String> {
        let listener = TcpListener::bind(CALLBACK_ADDR).map_err(|error| {
            format!("could not start wallet callback listener on {CALLBACK_ADDR}: {error}")
        })?;
        let (sender, receiver) = mpsc::channel();
        let challenge = challenge.to_string();
        let callback_challenge = challenge.clone();
        thread::Builder::new()
            .name("veloren-wallet-bridge".to_string())
            .spawn(move || {
                for stream in listener.incoming() {
                    match stream {
                        Ok(stream) => match handle_callback(stream, &callback_challenge) {
                            Ok(Some(link)) => {
                                if sender.send(link).is_err() {
                                    break;
                                }
                                break;
                            },
                            Ok(None) => {},
                            Err(error) => warn!(?error, "wallet callback request failed"),
                        },
                        Err(error) => warn!(?error, "wallet callback connection failed"),
                    }
                }
            })
            .map_err(|error| format!("could not start wallet callback thread: {error}"))?;

        let url = format!(
            "http://{CALLBACK_ADDR}/?challenge={}",
            percent_encode(&challenge)
        );
        open::that_detached(url)
            .map_err(|error| format!("could not open wallet bridge: {error}"))?;
        info!("opened local wallet bridge page");
        Ok(Self { receiver })
    }

    pub fn try_receive(&self) -> Option<WalletLink> { self.receiver.try_recv().ok() }
}

pub struct PurchaseBridge {
    receiver: Receiver<PropertyPurchase>,
}

pub struct VgldBridge {
    receiver: Receiver<VgldDeposit>,
}

impl VgldBridge {
    pub fn start(amount: u64, mint: &str, treasury: &str) -> Result<Self, String> {
        let listener = TcpListener::bind(VGLD_CALLBACK_ADDR).map_err(|error| {
            format!("could not start VGLD callback listener on {VGLD_CALLBACK_ADDR}: {error}")
        })?;
        let (sender, receiver) = mpsc::channel();
        thread::Builder::new()
            .name("veloren-vgld-bridge".to_string())
            .spawn(move || {
                for stream in listener.incoming() {
                    match stream {
                        Ok(mut stream) => match handle_vgld_callback(&mut stream) {
                            Ok(Some(deposit)) => {
                                if sender.send(deposit).is_err() {
                                    break;
                                }
                                break;
                            },
                            Ok(None) => {},
                            Err(error) => warn!(?error, "VGLD callback request failed"),
                        },
                        Err(error) => warn!(?error, "VGLD callback connection failed"),
                    }
                }
            })
            .map_err(|error| format!("could not start VGLD callback thread: {error}"))?;

        let url = format!(
            "http://{VGLD_CALLBACK_ADDR}/?amount={amount}&mint={}&treasury={}",
            percent_encode(mint),
            percent_encode(treasury)
        );
        open::that_detached(url).map_err(|error| format!("could not open VGLD bridge: {error}"))?;
        Ok(Self { receiver })
    }

    pub fn try_receive(&self) -> Option<VgldDeposit> { self.receiver.try_recv().ok() }
}

impl PurchaseBridge {
    pub fn start(parcel_id: &str, api_url: &str) -> Result<Self, String> {
        let listener = TcpListener::bind(PURCHASE_CALLBACK_ADDR).map_err(|error| {
            format!("could not start purchase callback listener on {PURCHASE_CALLBACK_ADDR}: {error}")
        })?;
        let (sender, receiver) = mpsc::channel();
        let parcel_id = parcel_id.to_string();
        let callback_parcel = parcel_id.clone();
        let api_url = api_url.to_string();
        thread::Builder::new()
            .name("veloren-property-purchase-bridge".to_string())
            .spawn(move || {
                for stream in listener.incoming() {
                    match stream {
                        Ok(stream) => match handle_purchase_callback(stream, &callback_parcel) {
                            Ok(Some(purchase)) => {
                                if sender.send(purchase).is_err() {
                                    break;
                                }
                                break;
                            },
                            Ok(None) => {},
                            Err(error) => warn!(?error, "property purchase callback failed"),
                        },
                        Err(error) => warn!(?error, "property purchase callback connection failed"),
                    }
                }
            })
            .map_err(|error| format!("could not start property purchase callback: {error}"))?;

        let url = format!(
            "http://{PURCHASE_CALLBACK_ADDR}/?parcel_id={}&api_url={}",
            percent_encode(&parcel_id),
            percent_encode(&api_url)
        );
        open::that_detached(url)
            .map_err(|error| format!("could not open property purchase bridge: {error}"))?;
        Ok(Self { receiver })
    }

    pub fn try_receive(&self) -> Option<PropertyPurchase> { self.receiver.try_recv().ok() }
}

fn handle_callback(mut stream: TcpStream, challenge: &str) -> Result<Option<WalletLink>, String> {
    let (mut request, header_end) = read_request_headers(&mut stream)?;
    let headers = String::from_utf8(request[..header_end].to_vec())
        .map_err(|error| format!("wallet callback headers were not UTF-8: {error}"))?;
    let body_start = header_end + 4;
    let method = headers.split_whitespace().next().unwrap_or_default();
    if method == "GET" {
        let page = BRIDGE_HTML.replace(CHALLENGE_PLACEHOLDER, challenge);
        write_html(&mut stream, &page)?;
        return Ok(None);
    }
    if method == "OPTIONS" {
        write_response(&mut stream, 204, "")?;
        return Ok(None);
    }
    if method != "POST" {
        write_response(&mut stream, 405, "method not allowed")?;
        return Ok(None);
    }
    let length = content_length(&headers)?;
    read_body(&mut stream, &mut request, body_start, length)?;
    let body = std::str::from_utf8(&request[body_start..body_start + length])
        .map_err(|error| format!("wallet callback body was not UTF-8: {error}"))?;
    let link = serde_json::from_str(body)
        .map_err(|error| format!("invalid wallet callback JSON: {error}"))?;
    write_response(&mut stream, 200, "wallet link received")?;
    Ok(Some(link))
}

fn handle_purchase_callback(
    mut stream: TcpStream,
    parcel_id: &str,
) -> Result<Option<PropertyPurchase>, String> {
    let (mut request, header_end) = read_request_headers(&mut stream)?;
    let headers = String::from_utf8(request[..header_end].to_vec())
        .map_err(|error| format!("purchase callback headers were not UTF-8: {error}"))?;
    let body_start = header_end + 4;
    let mut request_line = headers.lines().next().unwrap_or_default().split_whitespace();
    let method = request_line.next().unwrap_or_default();
    let target = request_line.next().unwrap_or_default();
    if method == "GET" {
        let page_parcel_id = query_parameter(target, "parcel_id").unwrap_or(parcel_id);
        let page = PURCHASE_HTML.replace("__VELOREN_PARCEL_ID__", &percent_encode(page_parcel_id));
        write_html(&mut stream, &page)?;
        return Ok(None);
    }
    if method == "OPTIONS" {
        write_response(&mut stream, 204, "")?;
        return Ok(None);
    }
    if method != "POST" {
        write_response(&mut stream, 405, "method not allowed")?;
        return Ok(None);
    }
    let length = content_length(&headers)?;
    read_body(&mut stream, &mut request, body_start, length)?;
    let purchase: PropertyPurchase =
        serde_json::from_slice(&request[body_start..body_start + length])
            .map_err(|error| format!("invalid purchase callback JSON: {error}"))?;
    if purchase.parcel_id != parcel_id {
        return Err("purchase callback parcel does not match the active purchase".to_string());
    }
    write_response(&mut stream, 200, "purchase received")?;
    Ok(Some(purchase))
}

fn handle_vgld_callback(stream: &mut TcpStream) -> Result<Option<VgldDeposit>, String> {
    let (mut request, header_end) = read_request_headers(stream)?;
    let headers = String::from_utf8(request[..header_end].to_vec())
        .map_err(|error| format!("VGLD callback headers were not UTF-8: {error}"))?;
    let body_start = header_end + 4;
    let mut request_line = headers.lines().next().unwrap_or_default().split_whitespace();
    let method = request_line.next().unwrap_or_default();
    let target = request_line.next().unwrap_or_default();
    if method == "GET" {
        let page = VGLD_HTML
            .replace("__VELOREN_AMOUNT__", &query_parameter(target, "amount").unwrap_or_default())
            .replace("__VELOREN_MINT__", &query_parameter(target, "mint").unwrap_or_default())
            .replace("__VELOREN_TREASURY__", &query_parameter(target, "treasury").unwrap_or_default());
        write_html(stream, &page)?;
        return Ok(None);
    }
    if method == "OPTIONS" {
        write_response(stream, 204, "")?;
        return Ok(None);
    }
    if method != "POST" {
        write_response(stream, 405, "method not allowed")?;
        return Ok(None);
    }
    let length = content_length(&headers)?;
    read_body(stream, &mut request, body_start, length)?;
    let deposit = serde_json::from_slice(&request[body_start..body_start + length])
        .map_err(|error| format!("invalid VGLD callback JSON: {error}"))?;
    write_response(stream, 200, "VGLD deposit received")?;
    Ok(Some(deposit))
}

fn read_request_headers(stream: &mut TcpStream) -> Result<(Vec<u8>, usize), String> {
    let mut request = Vec::new();
    loop {
        let mut chunk = [0; 1024];
        let read = stream.read(&mut chunk).map_err(|error| error.to_string())?;
        if read == 0 {
            return Err("callback ended before headers were received".to_string());
        }
        request.extend_from_slice(&chunk[..read]);
        if let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
            return Ok((request, header_end));
        }
        if request.len() > 16 * 1024 {
            return Err("callback headers are too large".to_string());
        }
    }
}

fn read_body(
    stream: &mut TcpStream,
    request: &mut Vec<u8>,
    body_start: usize,
    length: usize,
) -> Result<(), String> {
    while request.len() < body_start + length {
        let mut chunk = [0; 1024];
        let read = stream.read(&mut chunk).map_err(|error| error.to_string())?;
        if read == 0 {
            return Err("callback ended before body was received".to_string());
        }
        request.extend_from_slice(&chunk[..read]);
    }
    Ok(())
}

fn content_length(headers: &str) -> Result<usize, String> {
    headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .ok_or_else(|| "callback did not include Content-Length".to_string())
}

fn write_html(stream: &mut TcpStream, page: &str) -> Result<(), String> {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nCache-Control: \
         no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        page.len(),
        page
    );
    stream.write_all(response.as_bytes()).map_err(|error| error.to_string())
}

fn write_response(stream: &mut TcpStream, status: u16, body: &str) -> Result<(), String> {
    let response = format!(
        "HTTP/1.1 {status} OK\r\nContent-Type: text/plain\r\nAccess-Control-Allow-Origin: \
         *\r\nAccess-Control-Allow-Methods: POST, OPTIONS\r\nAccess-Control-Allow-Headers: \
         content-type\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(response.as_bytes()).map_err(|error| error.to_string())
}

fn percent_encode(value: &str) -> String {
    value
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![byte as char]
            },
            byte => format!("%{byte:02X}").chars().collect(),
        })
        .collect()
}

fn query_parameter<'a>(target: &'a str, name: &str) -> Option<&'a str> {
    target
        .split_once('?')?
        .1
        .split('&')
        .find_map(|parameter| {
            let (key, value) = parameter.split_once('=')?;
            (key == name).then_some(value)
        })
}
