/// Header name lists and manipulation utilities shared across proxy modules.
use std::collections::HashMap;

/// Request headers that the proxy forwards upstream.
pub const PROXY_REQUEST_HEADERS: &[&str] = &[
    "range",
    "accept",
    "accept-encoding",
    "accept-language",
    "cache-control",
    "connection",
    "content-type",
    "cookie",
    "origin",
    "pragma",
    "referer",
    "user-agent",
];

/// Response headers that the proxy forwards to the client.
pub const PROXY_RESPONSE_HEADERS: &[&str] = &[
    "accept-ranges",
    "cache-control",
    "content-encoding",
    "content-language",
    "content-length",
    "content-range",
    "content-type",
    "etag",
    "expires",
    "last-modified",
    "server",
    "transfer-encoding",
    "vary",
];

/// Extract `h_*` prefixed parameters from a query-param map into a request-header map.
///
/// `h_user-agent=Mozilla%2F5.0` → `{"user-agent": "Mozilla/5.0"}`
pub fn extract_request_headers(params: &HashMap<String, String>) -> HashMap<String, String> {
    params
        .iter()
        .filter_map(|(k, v)| {
            k.strip_prefix("h_")
                .map(|name| (name.to_lowercase(), v.clone()))
        })
        .collect()
}

/// Extract `r_*` prefixed parameters (response header overrides).
///
/// `r_content-type=video%2Fmp2t` → `{"content-type": "video/mp2t"}`
pub fn extract_response_headers(params: &HashMap<String, String>) -> HashMap<String, String> {
    params
        .iter()
        .filter_map(|(k, v)| {
            k.strip_prefix("r_")
                .map(|name| (name.to_lowercase(), v.clone()))
        })
        .collect()
}

/// Build the `h_*` and `r_*` portion of a proxied query string from header maps.
pub fn headers_to_query_params(
    request_headers: &HashMap<String, String>,
    response_headers: &HashMap<String, String>,
) -> Vec<(String, String)> {
    let mut params = Vec::new();
    for (k, v) in request_headers {
        params.push((format!("h_{}", k), v.clone()));
    }
    for (k, v) in response_headers {
        // Only propagate rp_ (response-propagate) headers downstream
        params.push((format!("rp_{}", k), v.clone()));
    }
    params
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_request_headers() {
        let mut params = HashMap::new();
        params.insert("h_user-agent".to_string(), "VLC/3.0".to_string());
        params.insert("h_referer".to_string(), "https://example.com".to_string());
        params.insert("api_password".to_string(), "secret".to_string());

        let headers = extract_request_headers(&params);
        assert_eq!(
            headers.get("user-agent").map(|s| s.as_str()),
            Some("VLC/3.0")
        );
        assert_eq!(
            headers.get("referer").map(|s| s.as_str()),
            Some("https://example.com")
        );
        assert!(!headers.contains_key("api_password"));
    }
}
