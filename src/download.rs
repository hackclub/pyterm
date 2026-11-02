use std::{
    net::{IpAddr, Ipv4Addr},
    sync::OnceLock,
    time::Duration,
};

use anyhow::{Result, anyhow, bail};
use reqwest::{
    Client, Url,
    dns::{Addrs, Name, Resolve, Resolving},
    redirect::Policy,
};

use crate::github::{Cache, send_request};

static CLIENT: OnceLock<Client> = OnceLock::new();
static CACHE: Cache = OnceLock::new();

const REQUEST_TIMEOUT_SECONDS: u64 = 30;
const MAX_REDIRECTS: usize = 5;

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_multicast()
        || a == 0
        || (a == 100 && (64..128).contains(&b))
        || (a == 192 && b == 0 && c == 0)
        || (a == 198 && (18..20).contains(&b))
        || a >= 240)
}

// The server runs inside the cluster, so anything but the public internet
// (pod/service ranges, cloud metadata, loopback) must be unreachable.
fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_public_v4(ip),
        IpAddr::V6(ip) => {
            if let Some(v4) = ip.to_ipv4_mapped() {
                return is_public_v4(v4);
            }
            let s = ip.segments();
            if s[0] == 0x2002 {
                return is_public_v4(Ipv4Addr::new(
                    (s[1] >> 8) as u8,
                    s[1] as u8,
                    (s[2] >> 8) as u8,
                    s[2] as u8,
                ));
            }
            (s[0] & 0xe000) == 0x2000 && !(s[0] == 0x2001 && s[1] == 0x0db8)
        }
    }
}

// IP literals skip the resolver, so they're checked here as well.
fn check_url(url: &Url) -> Result<()> {
    if url.scheme() != "https" {
        bail!("only https urls are allowed");
    }
    if !url.username().is_empty() || url.password().is_some() {
        bail!("urls with credentials are not allowed");
    }
    let host = url.host_str().ok_or_else(|| anyhow!("url has no host"))?;
    match host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<IpAddr>()
    {
        Ok(ip) if !is_public(ip) => bail!("url host is not a public address"),
        _ => Ok(()),
    }
}

struct PublicResolver;

impl Resolve for PublicResolver {
    fn resolve(&self, name: Name) -> Resolving {
        Box::pin(async move {
            let addrs = tokio::net::lookup_host((name.as_str(), 0))
                .await?
                .filter(|x| is_public(x.ip()))
                .collect::<Vec<_>>();
            if addrs.is_empty() {
                return Err(format!("{} has no public addresses", name.as_str()).into());
            }
            Ok(Box::new(addrs.into_iter()) as Addrs)
        })
    }
}

fn download_client() -> Result<&'static Client> {
    if let Some(client) = CLIENT.get() {
        return Ok(client);
    }

    let client = Client::builder()
        .user_agent("HackClubPyterm (brendan@hackclub.com)")
        .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECONDS))
        .no_proxy()
        .dns_resolver(PublicResolver)
        .redirect(Policy::custom(|attempt| {
            if attempt.previous().len() >= MAX_REDIRECTS {
                attempt.error("too many redirects")
            } else if let Err(e) = check_url(attempt.url()) {
                attempt.error(e.to_string())
            } else {
                attempt.follow()
            }
        }))
        .build()?;

    Ok(CLIENT.get_or_init(|| client))
}

pub async fn get_download(url: &str) -> Result<String> {
    let url = Url::parse(url).map_err(|e| anyhow!("invalid dl_url {url}: {e}"))?;
    check_url(&url)?;
    send_request(download_client()?, &CACHE, url, None).await
}

pub fn download_title(url: &str) -> String {
    Url::parse(url)
        .ok()
        .and_then(|x| {
            x.path_segments()
                .and_then(|mut s| s.next_back().map(str::to_string))
                .filter(|s| !s.is_empty())
                .or_else(|| x.host_str().map(str::to_string))
        })
        .unwrap_or_else(|| "pyterm".to_string())
}
