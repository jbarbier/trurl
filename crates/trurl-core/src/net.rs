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
