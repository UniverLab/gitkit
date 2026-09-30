//! HTTP client construction for every request the updater makes.
//!
//! Release lookups and archive downloads go through [`agent`], which applies
//! `HTTPS_PROXY` / `HTTP_PROXY` / `NO_PROXY` (upper and lower case) the same
//! way the sibling UniverLab tools do. The resolution lives here instead of
//! ureq's own `proxy-from-env` feature for two reasons: ureq 2 supports no
//! `NO_PROXY` at all, and its env-proxy default flips with the feature set a
//! build enables, so the same URL would route differently between a default
//! and an `--all-features` build.

use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};

use super::update::{BinaryDownloader, ReleaseFetcher};

/// Total request timeout for the release-list lookup. An explicit command
/// must fail loudly, but it must never hang either.
pub(super) const RELEASE_TIMEOUT: Duration = Duration::from_secs(15);

/// Total request timeout for the archive download. A release binary is a few
/// MiB, so a slow link must not be mistaken for a dead one.
pub(super) const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);

/// Production release lookup. All network and HTTP-status handling lives
/// behind [`ReleaseFetcher`] so unit tests can use a deterministic fake.
pub struct RealFetcher;

impl ReleaseFetcher for RealFetcher {
    fn get(&self, url: &str) -> Result<String> {
        let response = agent(url, RELEASE_TIMEOUT)?
            .get(url)
            .set("User-Agent", "gitkit-update")
            .call()
            .map_err(|error| anyhow!(describe(&error)))?;
        if response.status() != 200 {
            bail!("HTTP {} from the GitHub releases API", response.status());
        }
        response
            .into_string()
            .map_err(|error| anyhow!("unparsable GitHub releases response: {error}"))
    }
}

/// Production binary downloader. The archive is decoded only after this seam
/// returns, keeping the updater tests entirely offline. Its errors are not
/// part of the exit-code contract: they surface as-is from the install flow.
pub struct RealDownloader;

impl BinaryDownloader for RealDownloader {
    fn download(&self, url: &str) -> Result<Vec<u8>> {
        let response = agent(url, DOWNLOAD_TIMEOUT)?
            .get(url)
            .set("User-Agent", "gitkit-update")
            .call()
            .map_err(|error| anyhow!("failed to download {url}: {error}"))?;
        if response.status() != 200 {
            bail!("Download failed: HTTP {}", response.status());
        }
        use std::io::Read;
        let mut bytes = Vec::new();
        response
            .into_reader()
            .read_to_end(&mut bytes)
            .context("failed to read update archive")?;
        Ok(bytes)
    }
}

/// Build the client for one request. Proxy handling is entirely ours:
/// `.try_proxy_from_env(false)` pins ureq's own env detection off in every
/// feature set — with `--all-features` its default would otherwise ignore
/// `NO_PROXY` — and [`proxy_value`] applies the environment instead.
pub(super) fn agent(url: &str, timeout: Duration) -> Result<ureq::Agent> {
    let (scheme, host) = authority(url);
    let mut builder = ureq::AgentBuilder::new()
        .timeout(timeout)
        .try_proxy_from_env(false);
    if let Some(value) = proxy_value(&|name| std::env::var(name).ok(), &scheme, &host) {
        let proxy = ureq::Proxy::new(&value)
            .map_err(|error| anyhow!("invalid proxy from the environment: {error}"))?;
        builder = builder.proxy(proxy);
    }
    Ok(builder.build())
}

/// Which proxy variable applies to this request. Scheme-specific first, then
/// `ALL_PROXY`, then `HTTP_PROXY` as the generic fallback — the order the
/// sibling tools resolve in. Upper and lower case names both count; an empty
/// or whitespace-only value is skipped so the next candidate wins.
fn proxy_value(
    lookup: &dyn Fn(&str) -> Option<String>,
    scheme: &str,
    host: &str,
) -> Option<String> {
    if is_no_proxy(lookup, host) {
        return None;
    }
    let candidates: &[&str] = if scheme == "https" {
        &[
            "HTTPS_PROXY",
            "https_proxy",
            "ALL_PROXY",
            "all_proxy",
            "HTTP_PROXY",
            "http_proxy",
        ]
    } else {
        &["HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy"]
    };
    candidates.iter().find_map(|name| {
        lookup(name)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
}

/// `NO_PROXY` / `no_proxy`, comma-separated and case-insensitive. Entries
/// are matched lowercased so one comparison rule covers every shape.
fn is_no_proxy(lookup: &dyn Fn(&str) -> Option<String>, host: &str) -> bool {
    let Some(entries) = lookup("NO_PROXY").or_else(|| lookup("no_proxy")) else {
        return false;
    };
    let host = host.to_ascii_lowercase();
    entries.split(',').any(|entry| {
        let entry = entry.trim().to_ascii_lowercase();
        no_proxy_entry_matches(&entry, &host)
    })
}

/// One lowercased `NO_PROXY` entry against a lowercased host. Bare domains
/// follow curl's rule — what reqwest applies for texforge and ghscaff — so
/// `github.com` matches that host *and* every subdomain, and a leading `.`
/// or `*` is equivalent to the bare name; a lone `*` matches everything.
/// Trailing `*` / `.` keep the prefix forms ureq 3 (demostage) accepts, so
/// a bypass listed by either engine's syntax is bypassed here too.
fn no_proxy_entry_matches(entry: &str, host: &str) -> bool {
    if entry == "*" {
        return true;
    }
    if entry.starts_with('.') || entry.starts_with("*.") {
        return domain_matches(entry.trim_start_matches(['.', '*']), host);
    }
    if entry.ends_with('*') {
        return host.starts_with(entry.trim_end_matches('*'));
    }
    if entry.ends_with('.') {
        return host.starts_with(entry);
    }
    domain_matches(entry, host)
}

/// `github.com` matches `github.com` and `api.github.com` but never
/// `xgithub.com` — the dot-boundary suffix rule curl and reqwest apply.
fn domain_matches(domain: &str, host: &str) -> bool {
    !domain.is_empty() && (host == domain || host.ends_with(&format!(".{domain}")))
}

/// Split `https://host[:port]/path` into `(scheme, lowercased host)`. Every
/// URL reaching this module is built by this crate (api.github.com, github.com,
/// a loopback test listener), so a tiny hand-rolled parser is enough — ureq 2
/// exports no URL type, and one is not worth a dependency here.
fn authority(url: &str) -> (String, String) {
    let (scheme, rest) = url.split_once("://").unwrap_or(("https", url));
    let host_port = rest.split('/').next().unwrap_or("");
    let host = host_port
        .rsplit_once('@')
        .map_or(host_port, |(_, host)| host);
    let host = host.split(':').next().unwrap_or(host);
    (scheme.to_ascii_lowercase(), host.to_ascii_lowercase())
}

/// One line, naming the cause, so `update check failed: …` satisfies the
/// exit-2 contract without an anyhow chain burying it. ureq 2 has no TLS
/// error kind: TLS failures arrive as `Transport` whose Display already
/// carries the rustls message on one line, so the fallback keeps it visible.
fn describe(error: &ureq::Error) -> String {
    match error {
        ureq::Error::Status(code, _) => format!("HTTP {code} from the GitHub releases API"),
        ureq::Error::Transport(transport) => match transport.kind() {
            ureq::ErrorKind::Dns => "DNS lookup failed".to_string(),
            ureq::ErrorKind::TooManyRedirects => "too many redirects".to_string(),
            _ => transport.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::io::Read as _;
    use std::net::TcpListener;
    use std::sync::mpsc;

    /// Every variable the client reads, saved before clearing so each test
    /// starts from a clean environment and the process env is restored on
    /// drop: one leaked proxy variable would misroute every later ureq test.
    struct EnvGuard(Vec<(&'static str, Option<std::ffi::OsString>)>);

    const ENV_VARS: [&str; 8] = [
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
        "ALL_PROXY",
        "all_proxy",
        "NO_PROXY",
        "no_proxy",
    ];

    impl EnvGuard {
        fn cleared() -> Self {
            let saved = ENV_VARS
                .iter()
                .map(|name| (*name, std::env::var_os(name)))
                .collect();
            for name in ENV_VARS {
                std::env::remove_var(name);
            }
            Self(saved)
        }

        fn set(&self, name: &str, value: &str) {
            std::env::set_var(name, value);
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (name, previous) in &self.0 {
                match previous {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }
    }

    fn lookup_from<'a>(
        vars: &'a [(&'static str, &'static str)],
    ) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            vars.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_string())
        }
    }

    fn proxy_for(
        vars: &[(&'static str, &'static str)],
        scheme: &str,
        host: &str,
    ) -> Option<String> {
        proxy_value(&lookup_from(vars), scheme, host)
    }

    fn no_proxy_for(vars: &[(&'static str, &'static str)], host: &str) -> bool {
        is_no_proxy(&lookup_from(vars), host)
    }

    #[test]
    fn proxy_value_prefers_scheme_specific_then_all_then_fallback() {
        let vars = [
            ("HTTPS_PROXY", "http://secure:1"),
            ("ALL_PROXY", "http://all:1"),
            ("HTTP_PROXY", "http://plain:1"),
        ];
        assert_eq!(
            proxy_for(&vars, "https", "api.github.com").as_deref(),
            Some("http://secure:1")
        );
        assert_eq!(
            proxy_for(&vars, "http", "example.com").as_deref(),
            Some("http://plain:1"),
            "the http branch never consults HTTPS_PROXY"
        );

        let vars = [("all_proxy", "http://all:2")];
        assert_eq!(
            proxy_for(&vars, "https", "api.github.com").as_deref(),
            Some("http://all:2"),
            "lower case counts"
        );

        let vars = [("HTTP_PROXY", "http://plain:3")];
        assert_eq!(
            proxy_for(&vars, "https", "api.github.com").as_deref(),
            Some("http://plain:3"),
            "HTTP_PROXY is the generic https fallback too"
        );

        let vars = [("HTTPS_PROXY", "http://secure:4")];
        assert_eq!(
            proxy_for(&vars, "http", "example.com"),
            None,
            "HTTPS_PROXY must not leak into plain http"
        );

        let vars = [("HTTPS_PROXY", "   "), ("ALL_PROXY", "http://all:5")];
        assert_eq!(
            proxy_for(&vars, "https", "api.github.com").as_deref(),
            Some("http://all:5"),
            "an empty value is skipped, the next candidate wins"
        );

        let vars = [("HTTPS_PROXY", " http://secure:6 ")];
        assert_eq!(
            proxy_for(&vars, "https", "api.github.com").as_deref(),
            Some("http://secure:6"),
            "surrounding whitespace is trimmed"
        );

        assert_eq!(
            proxy_for(&[], "https", "api.github.com"),
            None,
            "no variable means no proxy"
        );
    }

    #[test]
    fn no_proxy_entries_match_like_the_siblings() {
        assert!(no_proxy_for(
            &[("NO_PROXY", "api.github.com")],
            "api.github.com"
        ));
        assert!(
            !no_proxy_for(&[("NO_PROXY", "api.github.com")], "github.com"),
            "a narrower bypass entry never widens upward"
        );
        assert!(
            no_proxy_for(&[("NO_PROXY", "github.com")], "api.github.com"),
            "a bare domain covers subdomains — curl's rule, what reqwest
             applies for texforge and ghscaff"
        );
        assert!(
            no_proxy_for(&[("NO_PROXY", "github.com")], "github.com"),
            "the bare host itself"
        );
        assert!(
            !no_proxy_for(&[("NO_PROXY", "github.com")], "xgithub.com"),
            "subdomains match at a dot boundary only"
        );
        assert!(
            !no_proxy_for(&[("NO_PROXY", "github.com")], "github.com.example"),
            "the entry matches a suffix of the host, never a prefix"
        );
        assert!(
            no_proxy_for(&[("NO_PROXY", ".github.com")], "github.com"),
            "a leading dot is equivalent to the bare name"
        );
        assert!(no_proxy_for(
            &[("NO_PROXY", ".github.com")],
            "api.github.com"
        ));
        assert!(no_proxy_for(
            &[("NO_PROXY", "*.github.com")],
            "api.github.com"
        ));
        assert!(no_proxy_for(&[("NO_PROXY", "*")], "anything.example"));
        assert!(
            no_proxy_for(&[("no_proxy", "API.GitHub.com")], "api.github.com"),
            "the lower case variable counts and hosts compare case-insensitively"
        );
        assert!(
            no_proxy_for(
                &[("NO_PROXY", "localhost, api.github.com, .internal")],
                "API.GITHUB.COM"
            ),
            "comma lists may carry spaces"
        );
        assert!(
            !no_proxy_for(&[], "api.github.com"),
            "no variable means no bypass"
        );
        assert!(!no_proxy_for(&[("NO_PROXY", "")], "api.github.com"));
        assert!(no_proxy_for(&[("NO_PROXY", "api.*")], "api.github.com"));
        assert!(!no_proxy_for(&[("NO_PROXY", "github.com.")], "github.com"));
    }

    #[test]
    fn url_authority_splits_scheme_and_host() {
        assert_eq!(
            authority("https://api.github.com/repos/x"),
            ("https".to_string(), "api.github.com".to_string())
        );
        assert_eq!(
            authority("http://127.0.0.1:8080/p"),
            ("http".to_string(), "127.0.0.1".to_string())
        );
        assert_eq!(
            authority("HTTPS://API.GitHub.Com/releases"),
            ("https".to_string(), "api.github.com".to_string())
        );
    }

    /// The spec's proxy test: with `HTTPS_PROXY` set, the real client must
    /// hand the request to that proxy — proved here by a loopback listener
    /// that dies before it can answer, so the fetch fails and the listener's
    /// first line must be the CONNECT tunnel. Loopback only, no network.
    #[serial]
    #[test]
    fn env_https_proxy_routes_the_real_client() {
        let _env = EnvGuard::cleared();
        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback listener");
        let port = listener.local_addr().expect("listener address").port();
        let (tx, rx) = mpsc::channel();
        let proxy_thread = std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
            let mut request = Vec::new();
            let mut chunk = [0u8; 512];
            while !request.windows(2).any(|pair| pair == b"\r\n") {
                match stream.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => request.extend_from_slice(&chunk[..read]),
                }
            }
            let first_line = String::from_utf8_lossy(&request)
                .lines()
                .next()
                .unwrap_or_default()
                .to_string();
            let _ = tx.send(first_line);
            // Dropping the stream closes the connection: the fetch must fail.
        });

        _env.set("HTTPS_PROXY", &format!("http://127.0.0.1:{port}"));
        let error = RealFetcher
            .get("https://api.github.com/repos/UniverLab/gitkit/releases")
            .expect_err("the fake proxy cannot complete a GitHub request");
        assert!(!error.to_string().is_empty(), "the cause must be named");

        let line = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("the env proxy must receive the request");
        assert!(
            line.starts_with("CONNECT api.github.com:443"),
            "unexpected proxy request: {line}"
        );
        proxy_thread.join().expect("proxy thread");
    }

    /// A host listed in `NO_PROXY` bypasses the configured proxy entirely:
    /// the listener must see nothing, and the request fails on its own DNS
    /// lookup instead (`.invalid` never resolves, so no egress either). The
    /// bypass is listed as the bare domain `invalid`, so the curl-style
    /// subdomain match against `nonexistent.invalid` is what is under test.
    #[serial]
    #[test]
    fn no_proxy_env_bypasses_configured_proxy() {
        let _env = EnvGuard::cleared();
        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback listener");
        listener
            .set_nonblocking(true)
            .expect("nonblocking listener");
        let port = listener.local_addr().expect("listener address").port();
        _env.set("HTTPS_PROXY", &format!("http://127.0.0.1:{port}"));
        _env.set("NO_PROXY", "invalid");

        let error = RealFetcher
            .get("https://nonexistent.invalid/releases")
            .expect_err("RFC 2606 .invalid names never resolve");
        assert!(
            error.to_string().contains("DNS lookup failed"),
            "unexpected error: {error}"
        );

        std::thread::sleep(Duration::from_millis(100));
        match listener.accept() {
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            outcome => panic!("a NO_PROXY host must not reach the proxy: {outcome:?}"),
        }
    }

    #[serial]
    #[test]
    fn describe_names_each_cause() {
        let _env = EnvGuard::cleared();

        // A live 404: `Error::Status` cannot be built without a response.
        let responder = TcpListener::bind("127.0.0.1:0").expect("loopback listener");
        let port = responder.local_addr().expect("listener address").port();
        let writer = std::thread::spawn(move || {
            let Ok((mut stream, _)) = responder.accept() else {
                return;
            };
            let _ = std::io::Write::write_all(
                &mut stream,
                b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n",
            );
        });
        let url = format!("http://127.0.0.1:{port}/x");
        let error = agent(&url, Duration::from_secs(5))
            .expect("agent")
            .get(&url)
            .call()
            .expect_err("404 is an error");
        writer.join().expect("writer thread");
        assert!(describe(&error).contains("HTTP 404"), "unexpected: {error}");

        // Refused connection → transport fallback, one non-empty line.
        let refused = "http://127.0.0.1:1/x";
        let error = agent(refused, Duration::from_secs(5))
            .expect("agent")
            .get(refused)
            .call()
            .expect_err("nothing listens on the discard port");
        let cause = describe(&error);
        assert!(!cause.is_empty(), "the cause must be named");
        assert!(!cause.contains('\n'), "one line only: {cause}");

        // DNS failure names itself.
        let unreachable = "http://nonexistent.invalid/x";
        let error = agent(unreachable, Duration::from_secs(10))
            .expect("agent")
            .get(unreachable)
            .call()
            .expect_err("RFC 2606 .invalid names never resolve");
        assert!(
            describe(&error).contains("DNS lookup failed"),
            "unexpected: {error}"
        );
    }
}
