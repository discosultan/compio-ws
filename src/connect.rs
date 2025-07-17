use std::{io, result, sync::Arc};

use base64::{Engine, prelude::BASE64_STANDARD};
use compio::BufResult;
use compio::tls::{TlsConnector, TlsStream};
use compio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpStream,
};
use http::Uri;
use rand::Rng;
use rustls::ClientConfig;
use sha1::{Digest, Sha1};

use crate::{Client, Config};

#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    #[error("IO: {0}")]
    Io(#[from] io::Error),
    #[error("Invalid handshake response: {0}")]
    InvalidHandshakeResponse(String),
    #[error("Invalid Sec-WebSocket-Accept header")]
    InvalidWebSocketAcceptHeader,
    #[error("Attempted to connect with invalid URI scheme")]
    InvalidUriScheme,
}

pub type ConnectResult<T> = result::Result<T, ConnectError>;

impl Client<TlsStream<TcpStream>> {
    pub async fn connect_tls(uri: &Uri, config: &Config) -> ConnectResult<Self> {
        if uri.scheme_str() != Some("wss") {
            return Err(ConnectError::InvalidUriScheme);
        }

        let mut root_store = rustls::RootCertStore::empty();
        root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

        let tls_config = ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();

        let connector = TlsConnector::from(Arc::new(tls_config));

        // Connect, upgrade to TLS and perform WebSocket handshake.
        let stream = TcpStream::connect(format!(
            "{}:{}",
            uri.host().unwrap_or_default(),
            uri.port_u16().unwrap_or(443)
        ))
        .await?;
        TcpStream::set_nodelay(&stream, true)?;

        let stream = connector
            .connect(uri.host().unwrap_or_default(), stream)
            .await?;
        Ok(Self::new(handshake(stream, uri).await?, config))
    }
}

impl Client<TcpStream> {
    pub async fn connect_plain(uri: &Uri, config: &Config) -> ConnectResult<Self> {
        if uri.scheme_str() != Some("ws") {
            return Err(ConnectError::InvalidUriScheme);
        }

        // Connect and perform WebSocket handshake.
        let stream = TcpStream::connect(format!(
            "{}:{}",
            uri.host().unwrap_or_default(),
            uri.port_u16().unwrap_or(80)
        ))
        .await?;
        TcpStream::set_nodelay(&stream, true)?;

        Ok(Self::new(handshake(stream, uri).await?, config))
    }
}

/// Performs a WebSocket handshake on an existing TCP connection via HTTP 1.
async fn handshake<T>(mut stream: T, uri: &Uri) -> ConnectResult<T>
where
    T: AsyncRead + AsyncWrite,
{
    // Generate a random key for the handshake.
    let mut rng = rand::rng();
    let mut key_bytes = [0u8; 16];
    rng.fill(&mut key_bytes);
    let key = BASE64_STANDARD.encode(key_bytes);

    // Create the HTTP request for the handshake.
    let request = http_request(uri, &key);

    // Send the handshake request.
    let BufResult(result, _) = stream.write_all(request.into_bytes()).await;
    result?;

    // Read the response.
    let mut response = String::with_capacity(2048);
    loop {
        let line = read_line(&mut stream).await?;
        response.push_str(&line);
        // Empty line signals end of headers.
        if line == "\r\n" {
            break;
        }
    }

    // Verify the response status.
    if !response.starts_with("HTTP/1.1 101") {
        return Err(ConnectError::InvalidHandshakeResponse(response));
    }

    // Verify the server's accept key.
    let expected_accept = {
        let mut hasher = Sha1::new();
        hasher.update(format!("{key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11").as_bytes());
        BASE64_STANDARD.encode(hasher.finalize())
    };
    if !response
        .to_lowercase()
        .contains(&format!("Sec-WebSocket-Accept: {expected_accept}").to_lowercase())
    {
        return Err(ConnectError::InvalidWebSocketAcceptHeader);
    }

    Ok(stream)
}

fn http_request(uri: &Uri, key: &str) -> String {
    let host = if let Some(port) = uri.port_u16() {
        format!("{}:{port}", uri.host().unwrap_or_default())
    } else {
        uri.host().unwrap_or_default().to_string()
    };

    format!(
        "GET {} HTTP/1.1\r\n\
         Host: {host}\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Key: {key}\r\n\
         Sec-WebSocket-Version: 13\r\n\
         \r\n",
        uri.path_and_query()
            .map(ToString::to_string)
            .unwrap_or_default(),
    )
}

async fn read_line<T>(stream: &mut T) -> io::Result<String>
where
    T: AsyncRead,
{
    let mut line = Vec::new();
    let mut buf = Box::new([0u8; 1]);

    loop {
        // Read byte-by-byte.
        let BufResult(result, read_buf) = stream.read_exact(buf).await;

        let _ = result?;
        buf = read_buf;

        line.push(buf[0]);
        if line.ends_with(b"\r\n") {
            break;
        }
    }

    String::from_utf8(line).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Invalid UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_http_request() {
        let output = http_request(
            &Uri::from_static("ws://localhost:9001/runCase?case=1&agent=monoio-ws"),
            "dGhlIHNhbXBsZSBub25jZQ==",
        );
        assert_eq!(
            output,
            "GET /runCase?case=1&agent=monoio-ws HTTP/1.1\r\n\
            Host: localhost:9001\r\n\
            Upgrade: websocket\r\n\
            Connection: Upgrade\r\n\
            Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
            Sec-WebSocket-Version: 13\r\n\
            \r\n"
        )
    }
}
