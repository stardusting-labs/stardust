//! HTTPS delivery for an enabled integration.
//!
//! The agent is built only when a post is ready. Idle checks with every
//! integration off return before that.

use stardust_core::outbound::{ip_is_blocked, public_https, DeliveryNote, PreparedPost};
use std::io::Read;
use std::net::{SocketAddr, ToSocketAddrs};
use std::time::Duration;

const BODY_LIMIT: u64 = 65_536;

pub struct HttpReply {
    pub status: u16,
    pub body: Vec<u8>,
}

pub fn deliver(notes: &[DeliveryNote]) {
    let mut ready = Vec::new();
    for note in notes {
        match note {
            DeliveryNote::Ready(post) => ready.push(post),
            DeliveryNote::Missing { id, what } => {
                eprintln!("stardust: integration {id} is missing a {what}");
            }
            DeliveryNote::Refused { id } => {
                eprintln!("stardust: integration {id} destination was refused");
            }
        }
    }
    if ready.is_empty() {
        return;
    }
    for post in ready {
        match send_post(post) {
            Ok(status) => eprintln!("stardust: posted to {} (status {status})", post.host),
            Err(_) => eprintln!("stardust: post to {} failed", post.host),
        }
    }
}

pub fn https_post(
    url: &str,
    content_type: &str,
    extra_headers: &[(String, String)],
    body: &[u8],
) -> Result<HttpReply, &'static str> {
    let checked = public_https(url).map_err(|_| "destination was refused")?;
    let port = port_of(url);
    let addrs = resolve_public(&checked.host, port)?;
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(10))
        .redirects(0)
        .resolver(PinnedAddrs(addrs))
        .build();
    let mut request = agent.post(url).set("Content-Type", content_type);
    for (name, value) in extra_headers {
        request = request.set(name, value);
    }
    match request.send_bytes(body) {
        Ok(response) => read_reply(response.status(), response),
        Err(ureq::Error::Status(status, response)) => read_reply(status, response),
        Err(_) => Err("request failed"),
    }
}

fn send_post(post: &PreparedPost) -> Result<u16, &'static str> {
    let headers: Vec<(String, String)> = post
        .headers
        .iter()
        .filter(|(name, _)| name != "Content-Type")
        .cloned()
        .collect();
    let reply = https_post(&post.url, "application/json", &headers, &post.body)?;
    let _ = reply.body;
    Ok(reply.status)
}

fn read_reply(status: u16, response: ureq::Response) -> Result<HttpReply, &'static str> {
    let mut body = Vec::new();
    response
        .into_reader()
        .take(BODY_LIMIT)
        .read_to_end(&mut body)
        .map_err(|_| "request failed")?;
    Ok(HttpReply { status, body })
}

struct PinnedAddrs(Vec<SocketAddr>);

impl ureq::Resolver for PinnedAddrs {
    fn resolve(&self, _netloc: &str) -> std::io::Result<Vec<SocketAddr>> {
        Ok(self.0.clone())
    }
}

pub fn resolve_public(host: &str, port: u16) -> Result<Vec<SocketAddr>, &'static str> {
    let looked_up = (host, port)
        .to_socket_addrs()
        .map_err(|_| "could not resolve destination")?;
    let mut addrs = Vec::new();
    for addr in looked_up {
        if ip_is_blocked(addr.ip()) {
            return Err("destination address was refused");
        }
        addrs.push(addr);
    }
    if addrs.is_empty() {
        return Err("could not resolve destination");
    }
    Ok(addrs)
}

fn port_of(url: &str) -> u16 {
    let rest = url.strip_prefix("https://").unwrap_or(url);
    let host_port = rest.split('/').next().unwrap_or("");
    let port = if let Some(inner) = host_port.strip_prefix('[') {
        inner.split_once("]:").map(|(_, port)| port)
    } else {
        host_port.split_once(':').map(|(_, port)| port)
    };
    port.and_then(|text| text.parse().ok()).unwrap_or(443)
}

#[cfg(test)]
mod tests {
    use super::{port_of, resolve_public};

    #[test]
    fn loopback_destinations_are_refused_before_a_request() {
        assert!(resolve_public("127.0.0.1", 443).is_err());
        assert!(resolve_public("localhost", 443).is_err());
        assert_eq!(port_of("https://helloorbit.app/hooks"), 443);
        assert_eq!(port_of("https://helloorbit.app:8443/hooks"), 8443);
    }
}
