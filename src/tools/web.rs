use std::{env, time::Duration};

use super::*;

#[derive(Deserialize)]
struct WebSearchArgs {
    query: String,
    #[serde(default)]
    max_results: Option<usize>,
}

const TAVILY_SEARCH_URL: &str = "https://api.tavily.com/search";
/// The no-JavaScript page: the plain `html/` endpoint answers a real browser
/// with a bot challenge, `lite/` still serves the results as ordinary markup.
const DUCKDUCKGO_SEARCH_URL: &str = "https://lite.duckduckgo.com/lite/";
const MAX_WEB_RESULTS: usize = 10;

/// Search the web and format the top results: Tavily when a key is set, the
/// DuckDuckGo HTML endpoint otherwise, so search works out of the box.
///
/// Uses a blocking HTTP client because tool execution runs on
/// `spawn_blocking`, not on the async runtime.
pub(super) fn web_search(arguments: &str, api_key: Option<&str>) -> Result<String> {
    let args: WebSearchArgs = parse(arguments)?;
    let query = args.query.trim();
    if query.is_empty() {
        bail!("query cannot be empty");
    }
    let max_results = args.max_results.unwrap_or(5).clamp(1, MAX_WEB_RESULTS);
    match api_key.map(str::trim).filter(|key| !key.is_empty()) {
        Some(api_key) => tavily_search(query, max_results, api_key),
        None => duckduckgo_search(query, max_results),
    }
}

/// Tavily, when the user configured a key.
fn tavily_search(query: &str, max_results: usize, api_key: &str) -> Result<String> {
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build()
        .context("creating the search client")?;
    let response = client
        .post(TAVILY_SEARCH_URL)
        .bearer_auth(api_key)
        .json(&json!({ "query": query, "max_results": max_results }))
        .send()
        .context("contacting Tavily")?;
    let status = response.status();
    let body: Value = response.json().context("reading the Tavily response")?;
    if !status.is_success() {
        let message = body
            .get("detail")
            .and_then(Value::as_str)
            .unwrap_or("no detail");
        bail!("Tavily returned {status}: {message}");
    }
    let results = body
        .get("results")
        .and_then(Value::as_array)
        .context("the Tavily response has no results")?;
    if results.is_empty() {
        return Ok(format!("{query}\n(no results)"));
    }
    let mut output = format!("{query}\n");
    for (index, result) in results.iter().enumerate() {
        let title = result
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("(untitled)");
        let url = result
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let snippet = result
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default();
        output.push_str(&format!(
            "\n{}. {title}\n{url}\n{}\n",
            index + 1,
            snippet.trim()
        ));
    }
    Ok(truncate_output(output))
}

/// DuckDuckGo's HTML endpoint: no key, no quota. It answers with ordinary
/// markup rather than JSON, so the results are read out of the page.
fn duckduckgo_search(query: &str, max_results: usize) -> Result<String> {
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        // DuckDuckGo serves its no-JavaScript page to a browser agent only.
        .user_agent("Mozilla/5.0 (compatible; yopes)")
        .build()
        .context("creating the search client")?;
    let response = client
        .post(DUCKDUCKGO_SEARCH_URL)
        .form(&[("q", query)])
        .send()
        .context("contacting DuckDuckGo")?;
    let status = response.status();
    let body = response.text().context("reading the DuckDuckGo response")?;
    if !status.is_success() {
        bail!("DuckDuckGo returned {status}");
    }
    let results = duckduckgo_results(&body, max_results);
    if results.is_empty() {
        return Ok(format!("{query}\n(no results)"));
    }
    let mut output = format!("{query}\n");
    for (index, (title, url, snippet)) in results.iter().enumerate() {
        output.push_str(&format!("\n{}. {title}\n{url}\n{snippet}\n", index + 1));
    }
    Ok(truncate_output(output))
}

/// `(title, url, snippet)` out of a DuckDuckGo result page.
///
/// The lite page repeats this shape per result: a link carrying
/// `class='result-link'` with the target in a plain `href`, and a
/// `class='result-snippet'` cell right after it.
fn duckduckgo_results(html: &str, max: usize) -> Vec<(String, String, String)> {
    let mut results = Vec::new();
    let mut cursor = 0usize;
    while results.len() < max {
        let Some(offset) = html[cursor..].find("result-link") else {
            break;
        };
        let class_at = cursor + offset;
        // The link's opening tag starts before the class attribute and ends
        // at the first `>` after it.
        let Some(tag_start) = html[..class_at].rfind("<a") else {
            cursor = class_at + 1;
            continue;
        };
        let Some(tag_end) = html[class_at..].find('>').map(|at| class_at + at) else {
            break;
        };
        let tag = &html[tag_start..tag_end];
        let Some(href) = attribute(tag, "href") else {
            cursor = tag_end + 1;
            continue;
        };
        let Some(url) = duckduckgo_url(&href) else {
            cursor = tag_end + 1;
            continue;
        };
        let title_end = html[tag_end..]
            .find("</a>")
            .map_or(html.len(), |at| tag_end + at);
        let title = clean_text(&html[tag_end + 1..title_end]);
        // The snippet follows the link in its own element.
        let snippet = html[title_end..]
            .find("result-snippet")
            .map(|at| {
                let start = title_end + at;
                let text_start = html[start..].find('>').map(|gt| start + gt + 1);
                let text_end = text_start
                    .and_then(|from| html[from..].find("</td>").map(|end| from + end))
                    .unwrap_or(start);
                clean_text(&html[text_start.unwrap_or(start)..text_end])
            })
            .unwrap_or_default();
        if !title.is_empty() {
            results.push((title, url, snippet));
        }
        cursor = title_end.max(class_at + 1);
    }
    results
}

/// One attribute value out of an HTML tag, single- or double-quoted.
fn attribute(tag: &str, name: &str) -> Option<String> {
    for quote in ['"', '\''] {
        let needle = format!("{name}={quote}");
        if let Some(at) = tag.find(&needle) {
            let start = at + needle.len();
            let end = tag[start..].find(quote)?;
            return Some(tag[start..start + end].to_owned());
        }
    }
    None
}

/// The real target of a DuckDuckGo result link: the page wraps it in a
/// redirect with the URL in `uddg`, percent-encoded.
fn duckduckgo_url(href: &str) -> Option<String> {
    let href = if let Some(rest) = href.split("uddg=").nth(1) {
        let encoded = rest.split('&').next().unwrap_or(rest);
        percent_decode(encoded)
    } else if href.starts_with("http") {
        href.to_owned()
    } else {
        return None;
    };
    href.starts_with("http").then_some(href)
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok();
                match hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                    Some(byte) => {
                        out.push(byte);
                        index += 3;
                    }
                    None => {
                        out.push(bytes[index]);
                        index += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Result text with its tags and the entities DuckDuckGo writes stripped.
fn clean_text(html: &str) -> String {
    let mut text = String::with_capacity(html.len());
    let mut in_tag = false;
    for character in html.chars() {
        match character {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => text.push(character),
            _ => {}
        }
    }
    text.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// How many bytes one `web_fetch` may pull.
const MAX_FETCH_BYTES: usize = 1_000_000;

#[cfg(test)]
mod search_tests {
    use super::{clean_text, duckduckgo_results, duckduckgo_url, percent_decode};

    /// The lite page's shape, trimmed to what the parser reads: two results,
    /// the first with a snippet, exactly as `lite.duckduckgo.com/lite/` serves
    /// them (single-quoted classes, plain `href`).
    const PAGE: &str = r#"<table border="0">
<tr><td valign="top">1.&nbsp;</td><td>
<a rel="nofollow" href="https://releases.rs/" class='result-link'>Rust Versions | <b>Rust</b> Changelogs</a></td></tr>
<tr><td>&nbsp;</td><td class='result-snippet'><b>Rust</b> Versions # Stable &amp; Beta</td></tr>
<tr><td valign="top">2.&nbsp;</td><td>
<a rel="nofollow" href="https://blog.rust-lang.org/" class='result-link'>The Rust Blog</a></td></tr>
<tr><td>&nbsp;</td><td class='result-snippet'>News from the team</td></tr>
</table>"#;

    #[test]
    fn duckduckgo_results_are_read_out_of_the_page() {
        let results = duckduckgo_results(PAGE, 5);
        assert_eq!(results.len(), 2, "{results:?}");
        assert_eq!(results[0].0, "Rust Versions | Rust Changelogs");
        assert_eq!(results[0].1, "https://releases.rs/");
        assert_eq!(results[0].2, "Rust Versions # Stable & Beta");
        assert_eq!(results[1].0, "The Rust Blog");
        assert_eq!(results[1].1, "https://blog.rust-lang.org/");
        assert_eq!(results[1].2, "News from the team");
        // `max` caps the rows even when the page holds more.
        assert_eq!(duckduckgo_results(PAGE, 1).len(), 1);
        // A page that answers with no results is not an error.
        assert!(duckduckgo_results("<html><body>nothing</body></html>", 5).is_empty());
    }

    #[test]
    fn redirect_links_and_entities_come_back_readable() {
        assert_eq!(percent_decode("a%20b%2Bc+d"), "a b+c d");
        // The lite page links straight out; the `uddg` redirect is still
        // understood, so a page that wraps links keeps working.
        assert_eq!(
            duckduckgo_url("//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2F"),
            Some("https://example.com/".to_owned())
        );
        // A plain link is used as given; a relative one is dropped.
        assert_eq!(
            duckduckgo_url("https://example.com/x"),
            Some("https://example.com/x".to_owned())
        );
        assert_eq!(duckduckgo_url("/l/?rut=1"), None);
        assert_eq!(clean_text(" a <b>b</b>  &amp; c "), "a b & c");
    }
}

#[derive(Deserialize)]
struct WebFetchArgs {
    url: String,
}

/// Fetch one page over HTTP GET and return its text: scripts, styles and
/// tags are gone, whitespace is squeezed to lines. Blocking, because tool
/// execution runs on `spawn_blocking`.
pub(super) fn web_fetch(arguments: &str) -> Result<String> {
    let args: WebFetchArgs = parse(arguments)?;
    let url = args.url.trim();
    if url.is_empty() {
        bail!("url cannot be empty");
    }
    if !url.starts_with("http://") && !url.starts_with("https://") {
        bail!("url must start with http:// or https://");
    }
    let parsed_url = reqwest::Url::parse(url).context("invalid URL")?;
    if is_blocked_destination(&parsed_url) {
        bail!("access to local or private network addresses is blocked");
    }
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(60))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 5 {
                return attempt.stop();
            }
            if is_blocked_destination(attempt.url()) {
                attempt.error("redirect to private or local network address blocked")
            } else {
                attempt.follow()
            }
        }))
        .build()
        .context("creating the fetch client")?;
    let response = client
        .get(url)
        .header("user-agent", format!("yopes/{}", env!("CARGO_PKG_VERSION")))
        .send()
        .with_context(|| format!("fetching {url}"))?;
    let status = response.status();
    if !status.is_success() {
        bail!("the server returned {status} for {url}");
    }
    let mut body_bytes = Vec::new();
    response
        .take((MAX_FETCH_BYTES + 1) as u64)
        .read_to_end(&mut body_bytes)
        .context("reading the response body")?;
    let body = String::from_utf8_lossy(&body_bytes);
    Ok(truncate_output(strip_html(&body)))
}

/// The page's text: `<script>` and `<style>` bodies dropped wholesale, all
/// other tags replaced by a space, the usual entities decoded, blank runs
/// collapsed. Good enough to read, deliberately not a browser engine.
pub(super) fn strip_html(body: &str) -> String {
    let body = drop_block(body, "script");
    let body = drop_block(&body, "style");
    let mut text = String::with_capacity(body.len());
    let mut at = 0;
    while at < body.len() {
        match body[at..].find('<') {
            None => {
                text.push_str(&body[at..]);
                break;
            }
            Some(open_rel) => {
                let open = at + open_rel;
                text.push_str(&body[at..open]);
                let Some(close_rel) = body[open..].find('>') else {
                    break;
                };
                // Tags often sit between words; a space keeps them separate.
                text.push(' ');
                at = open + close_rel + 1;
            }
        }
    }
    let text = decode_entities(&text);
    let mut clean = String::new();
    let mut blanks = 0;
    for line in text.lines() {
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        blanks = if line.is_empty() { blanks + 1 } else { 0 };
        if blanks <= 1 {
            clean.push_str(&line);
            clean.push('\n');
        }
    }
    clean.trim_end().to_owned()
}

/// Drop `<tag …>…</tag>` blocks: real pages nest scripts and styles everywhere
/// and their bodies are noise, not content.
fn drop_block(body: &str, tag: &str) -> String {
    let open_marker = format!("<{tag}");
    let close_marker = format!("</{tag}");
    let mut text = body.to_owned();
    loop {
        let lower = text.to_ascii_lowercase();
        let Some(start) = find_tag(&lower, &open_marker) else {
            return text;
        };
        let Some(close) = find_tag(&lower[start..], &close_marker).map(|rel| start + rel) else {
            // An unterminated block swallows everything after it.
            text.truncate(start);
            return text;
        };
        let end = close
            + close_marker.len()
            + lower[close + close_marker.len()..]
                .find('>')
                .map_or(0, |rel| rel + 1);
        let mut combined = String::with_capacity(text.len());
        combined.push_str(&text[..start]);
        combined.push(' ');
        combined.push_str(&text[end..]);
        text = combined;
    }
}

/// The offset of `<name` where the next character really opens an element:
/// `>`, `/` or whitespace, so `<scripts>` is never mistaken for `<script>`.
fn find_tag(hay: &str, marker: &str) -> Option<usize> {
    let mut at = 0;
    while let Some(rel) = hay[at..].find(marker) {
        let start = at + rel;
        at = start + marker.len();
        match hay[at..].chars().next() {
            Some('>') | Some('/') => return Some(start),
            Some(c) if c.is_ascii_whitespace() => return Some(start),
            _ => {}
        }
    }
    None
}

fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_owned();
    }
    // `&amp;` last, so encoded entities stay encoded.
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
}

fn is_blocked_destination(url: &reqwest::Url) -> bool {
    let Some(host) = url.host_str() else {
        return true;
    };
    let host = host.trim_matches('[').trim_matches(']');
    if host.eq_ignore_ascii_case("localhost")
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.ends_with(".internal")
    {
        return true;
    }
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return match ip {
            std::net::IpAddr::V4(ipv4) => {
                ipv4.is_loopback()
                    || ipv4.is_private()
                    || ipv4.is_link_local()
                    || ipv4.is_unspecified()
                    || (ipv4.octets()[0] == 100 && (ipv4.octets()[1] & 0xC0 == 64))
            }
            std::net::IpAddr::V6(ipv6) => {
                ipv6.is_loopback()
                    || ipv6.is_unspecified()
                    || ((ipv6.segments()[0] & 0xfe00) == 0xfc00)
                    || ((ipv6.segments()[0] & 0xffc0) == 0xfe80)
            }
        };
    }
    false
}
