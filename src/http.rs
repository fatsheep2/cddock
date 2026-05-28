use reqwest::{
    blocking::Client,
    header::{ACCEPT, AUTHORIZATION, HeaderMap},
};

pub const USER_AGENT: &str = "cddock/0.1.0";

pub fn client(context: &str) -> Result<Client, String> {
    Client::builder()
        .user_agent(USER_AGENT)
        .build()
        .map_err(|error| format!("Failed to create {context} HTTP client: {error}"))
}

pub fn github_client() -> Result<Client, String> {
    Client::builder()
        .user_agent(USER_AGENT)
        .default_headers(github_headers(std::env::var("GITHUB_TOKEN").ok())?)
        .build()
        .map_err(|error| format!("Failed to create GitHub HTTP client: {error}"))
}

fn github_headers(token: Option<String>) -> Result<HeaderMap, String> {
    let mut headers = HeaderMap::new();
    headers.insert(
        ACCEPT,
        "application/vnd.github+json"
            .parse()
            .map_err(|error| format!("Invalid GitHub accept header: {error}"))?,
    );
    if let Some(token) = token.filter(|token| !token.is_empty()) {
        headers.insert(
            AUTHORIZATION,
            format!("Bearer {token}")
                .parse()
                .map_err(|error| format!("Invalid GitHub token header: {error}"))?,
        );
    }
    Ok(headers)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_headers_include_auth_only_when_token_is_present() {
        let without_token = github_headers(None).expect("headers");
        assert!(without_token.get(AUTHORIZATION).is_none());

        let with_token = github_headers(Some("abc123".to_string())).expect("headers");
        assert_eq!(
            with_token
                .get(AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer abc123")
        );
    }
}
