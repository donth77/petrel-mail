//! Signing in with the provider rather than with a password.
//!
//! Microsoft has been retiring password sign-in for mail, and
//! `outlook.office365.com` offers `AUTH=XOAUTH2` beside `AUTH=PLAIN`; the
//! token that mechanism wants comes from here. Yahoo, AOL and iCloud advertise
//! XOAUTH2 too, and Google's flow is the same shape with different endpoints,
//! so this is written around a `Provider` rather than around Microsoft.
//!
//! Deliberately without a network. Everything here is a string: build a URL,
//! build a form, read a response. The desktop app opens the browser, listens
//! on loopback and does the POST, because those are its business and because
//! a module that reached the network could not be tested without one.
//!
//! The flow is authorization code with PKCE (RFC 7636), which is what a
//! desktop app must use: it cannot keep a client secret, and PKCE is what
//! replaces one. The verifier never leaves the machine; only its hash does,
//! so an authorization code stolen in transit is useless without the process
//! that started the exchange.

use base64::Engine as _;
use sha2::{Digest, Sha256};

/// Where a sign-in goes, and what it asks for.
///
/// The scopes are the narrow ones: mail over IMAP and submission over SMTP,
/// and nothing else. `offline_access` is what makes a refresh token come back
/// — without it the account stops working an hour later, which is the kind of
/// bug that looks like a server problem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Microsoft,
    Google,
}

impl Provider {
    pub fn authorize_endpoint(self) -> &'static str {
        match self {
            // `common` rather than a tenant id: personal accounts and work
            // accounts both sign in here, and the app is registered for both.
            Provider::Microsoft => "https://login.microsoftonline.com/common/oauth2/v2.0/authorize",
            Provider::Google => "https://accounts.google.com/o/oauth2/v2/auth",
        }
    }

    pub fn token_endpoint(self) -> &'static str {
        match self {
            Provider::Microsoft => "https://login.microsoftonline.com/common/oauth2/v2.0/token",
            Provider::Google => "https://oauth2.googleapis.com/token",
        }
    }

    pub fn scopes(self) -> &'static str {
        match self {
            // Exchange Online's own permissions, not Graph's. Graph cannot
            // speak IMAP, and asking for it would put a consent screen in
            // front of somebody for access this app has no use for.
            Provider::Microsoft => concat!(
                "https://outlook.office.com/IMAP.AccessAsUser.All ",
                "https://outlook.office.com/SMTP.Send ",
                "offline_access"
            ),
            // Google grants mail access as one scope covering both protocols.
            Provider::Google => "https://mail.google.com/",
        }
    }
}

/// One sign-in's secret and the hash that stands in for it.
#[derive(Debug, Clone)]
pub struct Pkce {
    /// Kept here, sent only at the end, never in a URL.
    pub verifier: String,
    /// The hash the authorize step carries.
    pub challenge: String,
    /// Ties the redirect back to the request that started it, so a browser
    /// arriving with somebody else's code is refused.
    pub state: String,
}

/// 32 bytes of randomness, base64url without padding.
///
/// Unpadded because the padding character is `=`, which has to be escaped in
/// a query string and is disallowed in a verifier by RFC 7636 anyway. Taking
/// it off here rather than escaping it later is one fewer thing to get wrong.
fn random_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("the system random source");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

impl Pkce {
    pub fn new() -> Self {
        let verifier = random_token();
        let digest = Sha256::digest(verifier.as_bytes());
        Pkce {
            challenge: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest),
            verifier,
            state: random_token(),
        }
    }
}

impl Default for Pkce {
    fn default() -> Self {
        Self::new()
    }
}

/// Percent-encoding for a query value.
///
/// Written out rather than pulled in: the set that must be escaped in a query
/// value is small and fixed, and the scopes carry `/` and `:` which some
/// encoders escape and some do not. Doing it here means the URL is the same
/// every time rather than depending on which crate version is resolved.
fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push_str("%20"),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The address to open in the person's own browser.
///
/// Their browser, not an embedded one: an embedded webview asking for a
/// Microsoft password is indistinguishable from one phishing for it, the
/// person cannot check the address bar, and their existing session and
/// passkeys are not there. Providers increasingly refuse it outright.
pub fn authorize_url(provider: Provider, client_id: &str, redirect: &str, pkce: &Pkce) -> String {
    let mut url = format!(
        "{}?client_id={}&response_type=code&redirect_uri={}&response_mode=query\
         &scope={}&code_challenge={}&code_challenge_method=S256&state={}",
        provider.authorize_endpoint(),
        encode(client_id),
        encode(redirect),
        encode(provider.scopes()),
        encode(&pkce.challenge),
        encode(&pkce.state),
    );
    if provider == Provider::Google {
        // Google only returns a refresh token when asked, and only the first
        // time unless consent is forced. Without this the account works for an
        // hour and then cannot renew.
        url.push_str("&access_type=offline&prompt=consent");
    }
    url
}

/// The form body that turns an authorization code into tokens.
///
/// Returned as pairs rather than a string so the caller encodes it the way its
/// HTTP client wants, and so a test can read it without parsing.
pub fn code_exchange_form<'a>(
    client_id: &'a str,
    redirect: &'a str,
    code: &'a str,
    verifier: &'a str,
) -> Vec<(&'static str, String)> {
    vec![
        ("client_id", client_id.to_string()),
        ("grant_type", "authorization_code".to_string()),
        ("code", code.to_string()),
        ("redirect_uri", redirect.to_string()),
        // The secret PKCE held back. The server hashes it and compares.
        ("code_verifier", verifier.to_string()),
    ]
}

/// The form body that renews an access token.
pub fn refresh_form<'a>(client_id: &'a str, refresh_token: &'a str) -> Vec<(&'static str, String)> {
    vec![
        ("client_id", client_id.to_string()),
        ("grant_type", "refresh_token".to_string()),
        ("refresh_token", refresh_token.to_string()),
    ]
}

/// What a token response carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tokens {
    pub access_token: String,
    /// Absent on some refreshes: a provider that is happy with the existing
    /// one simply does not send another, and the old one stays valid. Treating
    /// that as an error would sign somebody out every time it happened.
    pub refresh_token: Option<String>,
    /// Seconds from now, as the provider reported them.
    pub expires_in: i64,
}

/// Reads a token response, or says what the provider refused.
///
/// The error case is not decoration. OAuth failures come back as a 400 with a
/// JSON body naming the reason — `invalid_grant` for an expired refresh token,
/// `consent_required` for a tenant that has not approved the app — and those
/// are different problems with different answers. Collapsing them into "sign
/// in failed" would send somebody to check their password when the real answer
/// is that their administrator has to approve the application.
pub fn parse_tokens(body: &str) -> Result<Tokens, String> {
    let v: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("unreadable token response: {e}"))?;

    if let Some(error) = v.get("error").and_then(|e| e.as_str()) {
        let detail = v
            .get("error_description")
            .and_then(|d| d.as_str())
            .unwrap_or("");
        return Err(if detail.is_empty() {
            error.to_string()
        } else {
            format!("{error}: {detail}")
        });
    }

    let access_token = v
        .get("access_token")
        .and_then(|t| t.as_str())
        .ok_or("the response carried no access token")?
        .to_string();

    Ok(Tokens {
        access_token,
        refresh_token: v
            .get("refresh_token")
            .and_then(|t| t.as_str())
            .map(str::to_string),
        // A response without one is not worth refusing over; a short life is
        // safer than assuming a long one.
        expires_in: v.get("expires_in").and_then(|e| e.as_i64()).unwrap_or(3600),
    })
}

/// Pulls the code out of the redirect the browser follows.
///
/// `state` must match the one the request went out with. Without that check a
/// browser arriving with an authorization code from somewhere else would be
/// accepted, which is the whole of CSRF against this flow.
pub fn code_from_redirect(query: &str, expected_state: &str) -> Result<String, String> {
    let mut code = None;
    let mut state = None;
    let mut error = None;
    let mut description = None;
    for pair in query.trim_start_matches('?').split('&') {
        let Some((k, v)) = pair.split_once('=') else {
            continue;
        };
        let v = decode(v);
        match k {
            "code" => code = Some(v),
            "state" => state = Some(v),
            "error" => error = Some(v),
            "error_description" => description = Some(v),
            _ => {}
        }
    }
    if let Some(e) = error {
        return Err(match description {
            Some(d) if !d.is_empty() => format!("{e}: {d}"),
            _ => e,
        });
    }
    match (code, state) {
        (Some(_), Some(s)) if s != expected_state => {
            Err("the sign-in came back with the wrong state and was ignored".into())
        }
        (Some(c), Some(_)) => Ok(c),
        (Some(_), None) => Err("the sign-in came back without its state".into()),
        _ => Err("the sign-in came back without a code".into()),
    }
}

/// Undoes percent-encoding, and `+` for a space.
fn decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            // `get` rather than a slice: `%` followed by a multi-byte character
            // put the range inside it, and indexing there panics.
            b'%' if i + 2 < bytes.len() => match value
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
            {
                Some(b) => {
                    out.push(b);
                    i += 3;
                }
                None => {
                    out.push(bytes[i]);
                    i += 1;
                }
            },
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod decode_tests {
    use super::decode;

    #[test]
    fn a_percent_before_a_multibyte_character_does_not_panic() {
        assert_eq!(decode("%€"), "%€");
        assert_eq!(decode("%41+b%zz"), "A b%zz");
    }
}
