use anyhow::{bail, Result};
use rand::Rng;

/// RFC 2617 / RFC 2069 Digest authenticator.
pub struct DigestAuthenticator {
    username: String,
    password: String,
    realm: Option<String>,
    nonce: Option<String>,
    opaque: Option<String>,
    qop: Option<String>,
    nc: u32,
    cnonce: String,
}

impl DigestAuthenticator {
    pub fn new(username: impl Into<String>, password: impl Into<String>) -> Self {
        let cnonce = {
            let mut rng = rand::thread_rng();
            format!("{:016x}", rng.gen::<u64>())
        };
        Self {
            username: username.into(),
            password: password.into(),
            realm: None,
            nonce: None,
            opaque: None,
            qop: None,
            nc: 0,
            cnonce,
        }
    }

    /// Returns true if credentials are available (non-empty username).
    #[allow(dead_code)]
    pub fn has_credentials(&self) -> bool {
        !self.username.is_empty()
    }

    /// Returns true if a challenge has been parsed.
    pub fn is_challenged(&self) -> bool {
        self.nonce.is_some()
    }

    /// Parse the value of a `WWW-Authenticate: Digest ...` header.
    pub fn parse_challenge(&mut self, www_auth: &str) -> Result<()> {
        let rest = match www_auth.strip_prefix("Digest ") {
            Some(r) => r,
            None => bail!("Not a Digest challenge: {}", www_auth),
        };

        self.realm = extract_param(rest, "realm");
        self.nonce = extract_param(rest, "nonce");
        self.opaque = extract_param(rest, "opaque");
        self.qop = extract_param(rest, "qop");
        // Reset counter for new nonce.
        self.nc = 0;

        if self.nonce.is_none() {
            bail!("Digest challenge missing nonce");
        }
        Ok(())
    }

    /// Build and return the `Authorization: Digest ...` header value for the given
    /// method and URI. Returns `None` if the challenge has not been set yet.
    /// Increments the nonce counter on each call.
    pub fn apply(&mut self, method: &str, uri: &str) -> Option<String> {
        let nonce = self.nonce.as_deref()?;
        let realm = self.realm.as_deref().unwrap_or("");

        let ha1 = compute_md5(&format!("{}:{}:{}", self.username, realm, self.password));
        let ha2 = compute_md5(&format!("{}:{}", method, uri));

        self.nc += 1;
        let nc_str = format!("{:08x}", self.nc);

        let response = if self.qop.as_deref() == Some("auth") {
            compute_md5(&format!(
                "{}:{}:{}:{}:auth:{}",
                ha1, nonce, nc_str, self.cnonce, ha2
            ))
        } else {
            compute_md5(&format!("{}:{}:{}", ha1, nonce, ha2))
        };

        let mut parts = vec![
            format!("username=\"{}\"", self.username),
            format!("realm=\"{}\"", realm),
            format!("nonce=\"{}\"", nonce),
            format!("uri=\"{}\"", uri),
            format!("response=\"{}\"", response),
        ];

        if self.qop.as_deref() == Some("auth") {
            parts.push("qop=auth".to_string());
            parts.push(format!("nc={}", nc_str));
            parts.push(format!("cnonce=\"{}\"", self.cnonce));
        }

        if let Some(opaque) = &self.opaque {
            parts.push(format!("opaque=\"{}\"", opaque));
        }

        Some(format!("Digest {}", parts.join(", ")))
    }
}

fn compute_md5(input: &str) -> String {
    format!("{:x}", md5::compute(input))
}

/// Extract a quoted or unquoted parameter value from a Digest header.
fn extract_param(haystack: &str, name: &str) -> Option<String> {
    let needle_quoted = format!("{}=\"", name);
    let needle_plain = format!("{}=", name);

    if let Some(pos) = find_param_pos(haystack, &needle_quoted) {
        let start = pos + needle_quoted.len();
        let end = haystack[start..].find('"').map(|i| start + i)?;
        Some(haystack[start..end].to_string())
    } else if let Some(pos) = find_param_pos(haystack, &needle_plain) {
        let start = pos + needle_plain.len();
        let end = haystack[start..]
            .find([',', ' '])
            .map(|i| start + i)
            .unwrap_or(haystack.len());
        Some(haystack[start..end].trim().to_string())
    } else {
        None
    }
}

fn find_param_pos(haystack: &str, needle: &str) -> Option<usize> {
    let mut search_from = 0;
    while let Some(pos) = haystack[search_from..].find(needle) {
        let abs = search_from + pos;
        let ok = abs == 0 || {
            let prev = haystack.as_bytes()[abs - 1];
            prev == b',' || prev == b' ' || prev == b'\t'
        };
        if ok {
            return Some(abs);
        }
        search_from = abs + 1;
    }
    None
}
