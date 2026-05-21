use std::time::Duration;

use reqwest::redirect::Policy;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

pub fn client() -> reqwest::Client {
    builder()
        .build()
        .expect("reqwest client configuration is valid")
}

pub fn no_redirect_client() -> reqwest::Client {
    builder()
        .redirect(Policy::none())
        .build()
        .expect("reqwest client configuration is valid")
}

fn builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .connect_timeout(CONNECT_TIMEOUT)
}
