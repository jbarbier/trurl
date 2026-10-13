//! HTTP clients. Always build them through `client_builder()`, which makes
//! sure a TLS backend is ready (rustls with `ring` on Linux; the system TLS on
//! Windows needs nothing).

pub fn init_tls() {
    #[cfg(not(windows))]
    {
        // Fails only if a provider is already installed, which is fine.
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
}

pub fn client_builder() -> reqwest::ClientBuilder {
    init_tls();
    reqwest::Client::builder()
}

/// Read a reply body, but never more than `max` bytes: a broken or hostile
/// server must not be able to fill a 4 GB laptop's memory.
pub async fn read_limited(resp: reqwest::Response, max: usize) -> Result<Vec<u8>, String> {
    use futures_util::StreamExt;
    if resp.content_length().is_some_and(|n| n > max as u64) {
        return Err(format!("reply larger than {max} bytes"));
    }
    let mut buf = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.to_string())?;
        if buf.len() + chunk.len() > max {
            return Err(format!("reply larger than {max} bytes"));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

/// [`read_limited`], then parse it as JSON.
pub async fn json_limited<T: serde::de::DeserializeOwned>(
    resp: reqwest::Response,
    max: usize,
) -> Result<T, String> {
    let bytes = read_limited(resp, max).await?;
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}

/// True when the request never reached a server: no network, no DNS, refused.
/// Then the learner is told to connect, not that the site failed.
pub fn is_offline(e: &reqwest::Error) -> bool {
    e.is_connect() || (e.is_request() && !e.is_timeout() && !e.is_status())
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn a_refused_connection_counts_as_offline() {
        // Port 9 on localhost: nothing listens there.
        let e = super::client_builder()
            .build()
            .unwrap()
            .get("http://127.0.0.1:9/")
            .send()
            .await
            .unwrap_err();
        assert!(super::is_offline(&e));
    }
}
