//! The setup UI's one account.
//!
//! The first password comes from setup.toml as the image writer generated it.
//! A session that logged in with it can do nothing but set a new one, which is
//! stored as an argon2 hash in place of the original.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use argon2::Argon2;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};

pub const COOKIE: &str = "neslina_session";
pub const MIN_PASSWORD: usize = 12;
const SESSION_TTL: Duration = Duration::from_secs(12 * 3600);
/// Added to every failed login, so guessing over the LAN is slow.
pub const FAILURE_DELAY: Duration = Duration::from_secs(1);

#[derive(Clone)]
pub enum Credential {
    /// The image writer's password, in the clear on the stick.
    FirstLogin(String),
    /// The owner's own, as an argon2 PHC string.
    Hash(String),
}

impl Credential {
    pub fn verify(&self, password: &str) -> bool {
        match self {
            Credential::FirstLogin(p) => constant_time_eq(p.as_bytes(), password.as_bytes()),
            Credential::Hash(h) => PasswordHash::new(h)
                .is_ok_and(|parsed| Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok()),
        }
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    // SAFETY: getrandom fills at most N bytes.
    unsafe { libc::getrandom(b.as_mut_ptr().cast(), N, 0) };
    b
}

pub fn hash(password: &str) -> anyhow::Result<String> {
    let salt = SaltString::encode_b64(&random_bytes::<16>()).map_err(|e| anyhow::anyhow!("salt: {e}"))?;
    let hash =
        Argon2::default().hash_password(password.as_bytes(), &salt).map_err(|e| anyhow::anyhow!("hashing: {e}"))?;
    Ok(hash.to_string())
}

#[derive(Clone, Copy)]
pub struct Session {
    pub must_change_password: bool,
    expires: Instant,
}

#[derive(Default)]
pub struct Sessions {
    map: Mutex<HashMap<String, Session>>,
}

impl Sessions {
    pub fn create(&self, must_change_password: bool) -> String {
        let token: String = random_bytes::<32>().iter().map(|b| format!("{b:02x}")).collect();
        let mut map = self.map.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        map.retain(|_, s| s.expires > now);
        map.insert(token.clone(), Session { must_change_password, expires: now + SESSION_TTL });
        token
    }

    pub fn get(&self, token: &str) -> Option<Session> {
        let map = self.map.lock().unwrap_or_else(|e| e.into_inner());
        map.get(token).copied().filter(|s| s.expires > Instant::now())
    }

    pub fn remove(&self, token: &str) {
        self.map.lock().unwrap_or_else(|e| e.into_inner()).remove(token);
    }

    /// After a password change: this session stays, cleared to do anything;
    /// every other one ends.
    pub fn keep_only(&self, token: &str) {
        let mut map = self.map.lock().unwrap_or_else(|e| e.into_inner());
        map.retain(|t, _| t == token);
        if let Some(s) = map.get_mut(token) {
            s.must_change_password = false;
        }
    }
}

/// The session token from a Cookie header.
pub fn token_from(cookie_header: &str) -> Option<&str> {
    cookie_header.split(';').map(str::trim).find_map(|c| c.strip_prefix(COOKIE)?.strip_prefix('='))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_login_and_hash_both_verify() {
        let first = Credential::FirstLogin("correct horse".into());
        assert!(first.verify("correct horse"));
        assert!(!first.verify("correct hors"));
        let hashed = Credential::Hash(hash("battery staple!").unwrap());
        assert!(hashed.verify("battery staple!"));
        assert!(!hashed.verify("battery staple"));
        assert!(!Credential::Hash("not a hash".into()).verify(""));
    }

    #[test]
    fn sessions_and_password_change() {
        let s = Sessions::default();
        let a = s.create(true);
        let b = s.create(false);
        assert!(s.get(&a).unwrap().must_change_password);
        s.keep_only(&a);
        assert!(!s.get(&a).unwrap().must_change_password);
        assert!(s.get(&b).is_none());
        s.remove(&a);
        assert!(s.get(&a).is_none());
    }

    #[test]
    fn finds_the_cookie() {
        assert_eq!(token_from("a=1; neslina_session=abc; b=2"), Some("abc"));
        assert_eq!(token_from("neslina_sessionx=abc"), None);
        assert_eq!(token_from(""), None);
    }
}
