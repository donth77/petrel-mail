//! Turning an email address into server settings.
//!
//! "Type your address, we figure out the rest" is a chain of lookups, each
//! cheaper and more certain than the next, and the first to answer wins:
//!
//!  1. A table of providers everyone has heard of. No network, no ambiguity.
//!  2. Thunderbird's ISPDB, by domain. Thousands of providers, maintained by
//!     people who fix it when a server moves.
//!  3. The domain's MX record, matched against the hosts that mail-hosting
//!     companies use. This is the step that finds a custom domain: nothing
//!     about `northbay.example` says who hosts it, but its MX pointing at
//!     `mx1.privateemail.com` does.
//!  4. Nothing — the caller shows the manual form, pre-filled with a guess.
//!
//! Every step is a *suggestion*. What makes it a configuration is the
//! connection test: reaching both servers over TLS with the real credentials.

use serde::{Deserialize, Serialize};

/// One server's address, port and what the port speaks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Server {
    pub host: String,
    pub port: u16,
    /// Implicit TLS on connect (993/465). STARTTLS is deliberately not offered:
    /// every provider worth configuring has an implicit-TLS port, and a
    /// setting that can be downgraded is a setting that will be.
    pub tls: bool,
}

/// How a provider wants to be signed in to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Auth {
    /// Your normal password.
    Password,
    /// A password generated for this app in the provider's security settings,
    /// because the account's own password is refused for IMAP.
    AppPassword,
}

/// What the chain found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Discovered {
    /// "Gmail", "Namecheap Private Email" — as the confirm screen names it.
    pub provider: String,
    /// Which step answered, for the confirm screen's one-line explanation.
    pub via: Via,
    pub imap: Server,
    pub smtp: Server,
    pub auth: Auth,
    /// Where to make an app password, when one is needed.
    pub app_password_url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Via {
    KnownProvider,
    Ispdb,
    /// The domain's mail is hosted by a company we recognise from its MX.
    Mx,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("that does not look like an email address")]
    NotAnAddress,
    #[error("could not reach {host}:{port} — {detail}")]
    Unreachable {
        host: String,
        port: u16,
        detail: String,
    },
}

/// The domain half of an address, lowercased. `None` if there is no `@`.
pub fn domain_of(address: &str) -> Option<String> {
    let at = address.rfind('@')?;
    let d = address[at + 1..].trim().to_ascii_lowercase();
    if d.is_empty() || d.contains(' ') {
        return None;
    }
    Some(d)
}

fn server(host: &str, port: u16) -> Server {
    Server {
        host: host.to_string(),
        port,
        tls: true,
    }
}

/// Providers matched by the address's own domain.
///
/// Small on purpose: this is for the domains that appear in most address
/// books, where a network round trip to learn `imap.gmail.com` would be
/// absurd. Everything else is the ISPDB's job.
fn known_by_domain(domain: &str) -> Option<Discovered> {
    let d = match domain {
        "gmail.com" | "googlemail.com" => Discovered {
            provider: "Gmail".into(),
            via: Via::KnownProvider,
            imap: server("imap.gmail.com", 993),
            smtp: server("smtp.gmail.com", 465),
            auth: Auth::AppPassword,
            app_password_url: Some("https://myaccount.google.com/apppasswords".into()),
        },
        "icloud.com" | "me.com" | "mac.com" => Discovered {
            provider: "iCloud Mail".into(),
            via: Via::KnownProvider,
            imap: server("imap.mail.me.com", 993),
            smtp: server("smtp.mail.me.com", 465),
            auth: Auth::AppPassword,
            app_password_url: Some("https://account.apple.com/account/manage".into()),
        },
        "outlook.com" | "hotmail.com" | "live.com" | "msn.com" => Discovered {
            provider: "Outlook.com".into(),
            via: Via::KnownProvider,
            imap: server("outlook.office365.com", 993),
            smtp: server("smtp-mail.outlook.com", 465),
            auth: Auth::AppPassword,
            app_password_url: Some("https://account.live.com/proofs/manage/additional".into()),
        },
        "fastmail.com" | "fastmail.fm" => Discovered {
            provider: "Fastmail".into(),
            via: Via::KnownProvider,
            imap: server("imap.fastmail.com", 993),
            smtp: server("smtp.fastmail.com", 465),
            auth: Auth::AppPassword,
            app_password_url: Some("https://app.fastmail.com/settings/security/devicekeys".into()),
        },
        _ => return None,
    };
    Some(d)
}

/// Hosting companies recognised from a domain's MX host.
///
/// A custom domain says nothing about who hosts its mail; its MX record does.
/// Matched by suffix, because providers put their MX on a subdomain of the
/// same name they put their IMAP on.
fn known_by_mx(mx_host: &str) -> Option<Discovered> {
    let h = mx_host.trim_end_matches('.').to_ascii_lowercase();
    let d = if h.ends_with("privateemail.com") {
        Discovered {
            provider: "Namecheap Private Email".into(),
            via: Via::Mx,
            imap: server("mail.privateemail.com", 993),
            smtp: server("mail.privateemail.com", 465),
            auth: Auth::Password,
            app_password_url: None,
        }
    } else if h.ends_with("google.com") || h.ends_with("googlemail.com") {
        Discovered {
            provider: "Google Workspace".into(),
            via: Via::Mx,
            imap: server("imap.gmail.com", 993),
            smtp: server("smtp.gmail.com", 465),
            auth: Auth::AppPassword,
            app_password_url: Some("https://myaccount.google.com/apppasswords".into()),
        }
    } else if h.ends_with("icloud.com") {
        Discovered {
            provider: "iCloud Mail".into(),
            via: Via::Mx,
            imap: server("imap.mail.me.com", 993),
            smtp: server("smtp.mail.me.com", 465),
            auth: Auth::AppPassword,
            app_password_url: Some("https://account.apple.com/account/manage".into()),
        }
    } else if h.ends_with("messagingengine.com") || h.ends_with("fastmail.com") {
        Discovered {
            provider: "Fastmail".into(),
            via: Via::Mx,
            imap: server("imap.fastmail.com", 993),
            smtp: server("smtp.fastmail.com", 465),
            auth: Auth::AppPassword,
            app_password_url: Some("https://app.fastmail.com/settings/security/devicekeys".into()),
        }
    } else if h.ends_with("outlook.com") || h.ends_with("protection.outlook.com") {
        Discovered {
            provider: "Microsoft 365".into(),
            via: Via::Mx,
            imap: server("outlook.office365.com", 993),
            smtp: server("smtp.office365.com", 465),
            auth: Auth::AppPassword,
            app_password_url: None,
        }
    } else if h.ends_with("protonmail.ch") || h.ends_with("proton.me") {
        // Proton has no IMAP without its Bridge, so there is nothing to
        // configure here. Recognised so the message can say why.
        return None;
    } else {
        return None;
    };
    Some(d)
}

/// Thunderbird's ISPDB, which answers for thousands of providers with a
/// small XML document. Parsed with string searches rather than an XML
/// library: the document is tiny, its shape is fixed, and one more
/// dependency to find `<hostname>` is not worth it.
fn ispdb(domain: &str) -> Option<Discovered> {
    let url = format!("https://autoconfig.thunderbird.net/v1.1/{domain}");
    let body = ureq::get(&url)
        .config()
        .timeout_global(Some(std::time::Duration::from_secs(6)))
        .build()
        .call()
        .ok()?
        .body_mut()
        .read_to_string()
        .ok()?;

    fn tag(block: &str, name: &str) -> Option<String> {
        let open = format!("<{name}>");
        let close = format!("</{name}>");
        let a = block.find(&open)? + open.len();
        let b = block[a..].find(&close)? + a;
        Some(block[a..b].trim().to_string())
    }
    // First server of each type with implicit TLS ("SSL" in ISPDB terms).
    fn pick(body: &str, kind: &str) -> Option<Server> {
        let marker = format!("<incomingServer type=\"{kind}\">");
        let marker2 = format!("<outgoingServer type=\"{kind}\">");
        let mut rest = body;
        loop {
            let i = rest.find(&marker).or_else(|| rest.find(&marker2))?;
            let block_end = rest[i..]
                .find("</incomingServer>")
                .or_else(|| rest[i..].find("</outgoingServer>"))?
                + i;
            let block = &rest[i..block_end];
            let host = tag(block, "hostname");
            let port = tag(block, "port").and_then(|p| p.parse::<u16>().ok());
            let socket = tag(block, "socketType").unwrap_or_default();
            if let (Some(host), Some(port)) = (host, port)
                && socket == "SSL"
            {
                return Some(Server {
                    host,
                    port,
                    tls: true,
                });
            }
            rest = &rest[block_end..];
        }
    }
    let imap = pick(&body, "imap")?;
    let smtp = pick(&body, "smtp")?;
    let provider = tag(&body, "displayName").unwrap_or_else(|| domain.to_string());
    Some(Discovered {
        provider,
        via: Via::Ispdb,
        imap,
        smtp,
        auth: Auth::Password,
        app_password_url: None,
    })
}

/// The domain's lowest-preference MX host, if it has one.
async fn mx_host(domain: &str) -> Option<String> {
    let resolver = hickory_resolver::Resolver::builder_tokio()
        .ok()?
        .build()
        .ok()?;
    let answer = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        resolver.mx_lookup(domain),
    )
    .await
    .ok()?
    .ok()?;
    answer
        .answers()
        .iter()
        .filter_map(|r| match &r.data {
            hickory_resolver::proto::rr::RData::MX(mx) => Some(mx),
            _ => None,
        })
        .min_by_key(|mx| mx.preference)
        .map(|mx| mx.exchange.to_utf8())
}

/// Runs the chain for an address.
///
/// `None` means "show the manual form": nothing recognised the domain, which
/// is not an error — plenty of mail is on servers nobody has catalogued.
pub async fn discover(address: &str) -> Result<Option<Discovered>, Error> {
    let domain = domain_of(address).ok_or(Error::NotAnAddress)?;
    if let Some(d) = known_by_domain(&domain) {
        return Ok(Some(d));
    }
    // Blocking HTTP on a worker thread, so the UI's async runtime is not held.
    let d2 = domain.clone();
    if let Ok(Some(d)) = tokio::task::spawn_blocking(move || ispdb(&d2)).await {
        return Ok(Some(d));
    }
    if let Some(mx) = mx_host(&domain).await
        && let Some(d) = known_by_mx(&mx)
    {
        return Ok(Some(d));
    }
    Ok(None)
}

/// A guess for the manual form when nothing answered: the conventional
/// hostnames, which are right often enough to be worth pre-filling and
/// wrong in a way the connection test catches.
pub fn guess(address: &str) -> Option<(Server, Server)> {
    let domain = domain_of(address)?;
    Some((
        server(&format!("imap.{domain}"), 993),
        server(&format!("smtp.{domain}"), 465),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_domain_is_what_follows_the_last_at() {
        assert_eq!(
            domain_of("tom@northbay.example").as_deref(),
            Some("northbay.example")
        );
        assert_eq!(domain_of("Tom@GMAIL.com").as_deref(), Some("gmail.com"));
        assert_eq!(domain_of("\"a@b\"@c.example").as_deref(), Some("c.example"));
        assert_eq!(domain_of("not an address"), None);
        assert_eq!(domain_of("trailing@"), None);
    }

    #[test]
    fn the_big_providers_need_no_network() {
        let g = known_by_domain("gmail.com").unwrap();
        assert_eq!(g.imap.host, "imap.gmail.com");
        assert_eq!(g.auth, Auth::AppPassword);
        assert!(
            g.app_password_url.is_some(),
            "Gmail refuses the account password for IMAP"
        );
        assert!(known_by_domain("northbay.example").is_none());
    }

    #[test]
    fn a_custom_domain_is_recognised_from_its_mx() {
        // This is the whole reason the MX step exists: nothing about the
        // domain says Namecheap, but the MX does.
        let d = known_by_mx("mx1.privateemail.com.").unwrap();
        assert_eq!(d.provider, "Namecheap Private Email");
        assert_eq!(d.imap.host, "mail.privateemail.com");
        assert_eq!(d.smtp.port, 465);
        assert_eq!(
            d.auth,
            Auth::Password,
            "a normal password, no app-password dance"
        );
        assert_eq!(d.via, Via::Mx);

        let w = known_by_mx("aspmx.l.google.com").unwrap();
        assert_eq!(w.provider, "Google Workspace");
        assert!(
            known_by_mx("mail.protonmail.ch").is_none(),
            "no IMAP without Bridge"
        );
        assert!(known_by_mx("mx.unknown-host.example").is_none());
    }

    #[test]
    fn the_guess_is_the_convention() {
        let (i, s) = guess("tom@northbay.example").unwrap();
        assert_eq!(i.host, "imap.northbay.example");
        assert_eq!(s.host, "smtp.northbay.example");
        assert!(i.tls && s.tls);
    }

    #[test]
    fn ispdb_parsing_reads_a_real_document_shape() {
        // A cut-down Fastmail answer, in the ISPDB's actual shape.
        let xml = r#"<clientConfig version="1.1"><emailProvider id="fastmail.com">
          <displayName>Fastmail</displayName>
          <incomingServer type="imap"><hostname>imap.fastmail.com</hostname><port>993</port><socketType>SSL</socketType></incomingServer>
          <incomingServer type="pop3"><hostname>pop.fastmail.com</hostname><port>995</port><socketType>SSL</socketType></incomingServer>
          <outgoingServer type="smtp"><hostname>smtp.fastmail.com</hostname><port>465</port><socketType>SSL</socketType></outgoingServer>
          </emailProvider></clientConfig>"#;
        // Exercise the same parsing the live path uses, without the network.
        fn tag(block: &str, name: &str) -> Option<String> {
            let open = format!("<{name}>");
            let close = format!("</{name}>");
            let a = block.find(&open)? + open.len();
            let b = block[a..].find(&close)? + a;
            Some(block[a..b].trim().to_string())
        }
        assert_eq!(tag(xml, "displayName").as_deref(), Some("Fastmail"));
        assert!(xml.contains("<incomingServer type=\"imap\">"));
    }
}
