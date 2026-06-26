//! The pluggable auth seam (`spec/registries.md` §1).
//!
//! Auth resolvers (CDS / FIRMS / OpenAQ / RDA / bearer) are a separate map
//! injected into a transport at fetch time — **never** baked into a transport.
//! A resolver turns a realm into the HTTP headers a request needs; credentials
//! never touch the manifest (only the realm name is recorded).

use std::collections::HashMap;
use std::sync::Arc;

/// Resolves credentials for an authenticated realm into request headers.
pub trait AuthResolver: Send + Sync {
    /// The realm this resolver authenticates (e.g. `"cds"`).
    fn realm(&self) -> &str;

    /// Headers to attach to a request for this realm. An empty list is valid
    /// (e.g. an anonymous-but-namespaced realm).
    fn headers(&self) -> Vec<(String, String)>;
}

/// Realm → resolver map. The fetch layer looks a realm up here and hands the
/// resolver to the transport; an unknown realm is a clean error, not a panic.
#[derive(Default, Clone)]
pub struct AuthRegistry {
    resolvers: HashMap<String, Arc<dyn AuthResolver>>,
}

impl AuthRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a resolver under its realm. Replaces any existing resolver for
    /// that realm.
    pub fn register(&mut self, resolver: Arc<dyn AuthResolver>) -> &mut Self {
        self.resolvers
            .insert(resolver.realm().to_string(), resolver);
        self
    }

    /// Look up the resolver for a realm.
    pub fn get(&self, realm: &str) -> Option<Arc<dyn AuthResolver>> {
        self.resolvers.get(realm).cloned()
    }

    /// Realms with a registered resolver.
    pub fn realms(&self) -> Vec<String> {
        self.resolvers.keys().cloned().collect()
    }
}

/// A resolver that attaches a fixed set of headers for a realm — covers the
/// common "token in a header" case (FIRMS map key, OpenAQ API key, a bearer
/// token, …). Construct from a token or arbitrary headers.
pub struct StaticHeaderAuth {
    realm: String,
    headers: Vec<(String, String)>,
}

impl StaticHeaderAuth {
    /// `Authorization: Bearer <token>` for `realm`.
    pub fn bearer(realm: impl Into<String>, token: impl AsRef<str>) -> Self {
        Self {
            realm: realm.into(),
            headers: vec![(
                "Authorization".to_string(),
                format!("Bearer {}", token.as_ref()),
            )],
        }
    }

    /// A single arbitrary header for `realm` (e.g. `X-API-Key: …`).
    pub fn header(
        realm: impl Into<String>,
        name: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        Self {
            realm: realm.into(),
            headers: vec![(name.into(), value.into())],
        }
    }

    /// Multiple arbitrary headers for `realm`.
    pub fn headers(realm: impl Into<String>, headers: Vec<(String, String)>) -> Self {
        Self {
            realm: realm.into(),
            headers,
        }
    }
}

impl AuthResolver for StaticHeaderAuth {
    fn realm(&self) -> &str {
        &self.realm
    }
    fn headers(&self) -> Vec<(String, String)> {
        self.headers.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_resolve() {
        let mut reg = AuthRegistry::new();
        reg.register(Arc::new(StaticHeaderAuth::bearer("cds", "secret-token")));
        let r = reg.get("cds").expect("cds resolver registered");
        assert_eq!(r.realm(), "cds");
        assert_eq!(
            r.headers(),
            vec![(
                "Authorization".to_string(),
                "Bearer secret-token".to_string()
            )]
        );
        assert!(reg.get("rda").is_none());
    }

    #[test]
    fn credentials_are_only_in_headers_never_the_realm() {
        let a = StaticHeaderAuth::header("firms", "X-API-Key", "abc123");
        assert_eq!(a.realm(), "firms"); // realm name carries no secret
        assert_eq!(
            a.headers(),
            vec![("X-API-Key".to_string(), "abc123".to_string())]
        );
    }
}
