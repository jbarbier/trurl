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

/// Why a request failed, in the terms a learner can act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetFailure {
    /// No route, no name lookup, refused: the computer is not online.
    Offline,
    /// It reached something, but the secure connection failed: on these
    /// laptops most often a wrong clock (dead CMOS battery), or a network that
    /// intercepts HTTPS. "Connect to the internet" would be the wrong advice.
    Tls,
    /// Anything else (a proxy that wants a login, a server error, a timeout).
    Other,
}

pub fn classify(e: &(dyn std::error::Error + 'static)) -> NetFailure {
    use std::io::ErrorKind as K;
    let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(e);
    let mut offline = false;
    let mut depth = 0;
    while let Some(err) = cur {
        // The outermost message carries the URL (a search for "ssl" must not
        // look like a TLS failure): read words only from the causes.
        let text = if depth == 0 {
            String::new()
        } else {
            err.to_string().to_ascii_lowercase()
        };
        depth += 1;
        if [
            "certificate",
            "tls",
            "ssl",
            "handshake",
            "schannel",
            "unknownissuer",
            "badsignature",
        ]
        .iter()
        .any(|w| text.contains(w))
        {
            return NetFailure::Tls;
        }
        if let Some(io) = err.downcast_ref::<std::io::Error>()
            && matches!(
                io.kind(),
                K::ConnectionRefused
                    | K::NetworkUnreachable
                    | K::HostUnreachable
                    | K::NetworkDown
                    | K::AddrNotAvailable
            )
        {
            offline = true;
        }
        if text.contains("dns error")
            || text.contains("failed to lookup address")
            || text.contains("no such host")
            || text.contains("name or service not known")
            || text.contains("temporary failure in name resolution")
        {
            offline = true;
        }
        cur = err.source();
    }
    if offline {
        NetFailure::Offline
    } else {
        NetFailure::Other
    }
}

/// A proxy that would not pass the request on (407, a refused tunnel).
pub fn is_proxy_refusal(e: &(dyn std::error::Error + 'static)) -> bool {
    let mut top = e.to_string().to_ascii_lowercase();
    // The outer message carries the URL: only its fixed words count.
    if let Some(i) = top.find("for url (") {
        top.truncate(i);
    }
    let hit = |t: &str| t.contains("unsuccessful tunnel") || t.contains("proxy authentication");
    if hit(&top) {
        return true;
    }
    let mut cur = e.source();
    while let Some(c) = cur {
        if hit(&c.to_string().to_ascii_lowercase()) {
            return true;
        }
        cur = c.source();
    }
    false
}

/// True when the request never reached a server: no network, no DNS, refused.
/// Then the learner is told to connect, not that the site failed.
pub fn is_offline(e: &reqwest::Error) -> bool {
    e.is_connect() && classify(e) == NetFailure::Offline
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct Wrap(&'static str, Option<Box<dyn std::error::Error + 'static>>);
    impl std::fmt::Display for Wrap {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(self.0)
        }
    }
    impl std::error::Error for Wrap {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            self.1.as_deref()
        }
    }

    #[test]
    fn a_wrong_clock_is_not_told_to_connect_to_the_internet() {
        let tls = Wrap(
            "error sending request",
            Some(Box::new(Wrap("invalid peer certificate: Expired", None))),
        );
        assert_eq!(classify(&tls), NetFailure::Tls);
        let refused = Wrap(
            "error sending request",
            Some(Box::new(std::io::Error::from(
                std::io::ErrorKind::ConnectionRefused,
            ))),
        );
        assert_eq!(classify(&refused), NetFailure::Offline);
        let dns = Wrap(
            "client error (Connect)",
            Some(Box::new(Wrap(
                "dns error: failed to lookup address information",
                None,
            ))),
        );
        assert_eq!(classify(&dns), NetFailure::Offline);
        let proxy = Wrap(
            "unsuccessful tunnel (407 Proxy Authentication Required)",
            None,
        );
        assert_eq!(classify(&proxy), NetFailure::Other);
        assert!(is_proxy_refusal(&proxy));
        let dropped = Wrap(
            "error sending request for url (https://huggingface.co/a/b/resolve/main/proxy-tunnel.gguf)",
            Some(Box::new(Wrap(
                "connection closed before message completed",
                None,
            ))),
        );
        assert!(!is_proxy_refusal(&dropped), "a drop is not a proxy refusal");
        // Offline, searching for "ssl": the URL in the outer message is not a TLS failure.
        let search = Wrap(
            "error sending request for url (https://huggingface.co/api/models?search=ssl-tls-certificate)",
            Some(Box::new(Wrap(
                "dns error: failed to lookup address information",
                None,
            ))),
        );
        assert_eq!(classify(&search), NetFailure::Offline);
    }

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
