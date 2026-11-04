//! Secret values sealed at rest (PLAN P8.4, ADR-026).
//!
//! A sealed value is `enc:v1:` followed by base64url of a 12-byte random nonce and the AES-256-GCM
//! ciphertext with its tag. The variable's name is the associated data, so a sealed value only
//! opens under the name it was sealed for.
//!
//! The key is 32 random bytes, written as base64. It comes from `SNAPREQ_SECRET_KEY`, or from the
//! file `secret.key` in the server's data directory (created by the server; [`load_or_create`]).

use std::path::Path;

use base64::Engine as _;
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};
use ring::rand::{SecureRandom, SystemRandom};

/// Prefix of sealed values.
pub const PREFIX: &str = "enc:v1:";
/// Environment variable holding the key (base64).
pub const KEY_ENV: &str = "SNAPREQ_SECRET_KEY";
/// Key file name in the server's data directory.
pub const KEY_FILE: &str = "secret.key";

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::URL_SAFE_NO_PAD;

/// The key that seals and opens secret values.
#[derive(Clone)]
pub struct SecretKey([u8; 32]);

impl std::fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretKey(<redacted>)")
    }
}

/// Whether a stored value is sealed.
pub fn is_sealed(value: &str) -> bool {
    value.starts_with(PREFIX)
}

impl SecretKey {
    /// A new random key.
    pub fn generate() -> Result<Self, String> {
        let mut k = [0u8; 32];
        SystemRandom::new()
            .fill(&mut k)
            .map_err(|_| "the system random generator failed".to_owned())?;
        Ok(Self(k))
    }

    /// Reads a key written by [`SecretKey::encode`] (standard or URL-safe base64).
    pub fn decode(text: &str) -> Result<Self, String> {
        let text = text.trim();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(text)
            .or_else(|_| B64.decode(text.trim_end_matches('=')))
            .map_err(|_| "the secret key is not base64".to_owned())?;
        <[u8; 32]>::try_from(bytes.as_slice())
            .map(Self)
            .map_err(|_| "the secret key must be 32 bytes".to_owned())
    }

    /// The key as standard base64.
    pub fn encode(&self) -> String {
        base64::engine::general_purpose::STANDARD.encode(self.0)
    }

    fn aead(&self) -> LessSafeKey {
        // A 32-byte key always fits AES-256.
        #[allow(clippy::expect_used)]
        LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &self.0).expect("32-byte key"))
    }

    /// Seals `value` for variable `name`.
    pub fn seal(&self, name: &str, value: &str) -> Result<String, String> {
        let mut nonce = [0u8; NONCE_LEN];
        SystemRandom::new()
            .fill(&mut nonce)
            .map_err(|_| "the system random generator failed".to_owned())?;
        let mut data = value.as_bytes().to_vec();
        self.aead()
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(name.as_bytes()),
                &mut data,
            )
            .map_err(|_| "sealing failed".to_owned())?;
        let mut out = nonce.to_vec();
        out.extend_from_slice(&data);
        Ok(format!("{PREFIX}{}", B64.encode(out)))
    }

    /// Opens a sealed value of variable `name`; values that are not sealed come back as they are.
    pub fn open(&self, name: &str, stored: &str) -> Result<String, String> {
        let Some(body) = stored.strip_prefix(PREFIX) else {
            return Ok(stored.to_owned());
        };
        let bad = || format!("the value of `{name}` cannot be opened with this secret key");
        let mut bytes = B64.decode(body).map_err(|_| bad())?;
        if bytes.len() < NONCE_LEN {
            return Err(bad());
        }
        let mut data = bytes.split_off(NONCE_LEN);
        let nonce = Nonce::try_assume_unique_for_key(&bytes).map_err(|_| bad())?;
        let plain = self
            .aead()
            .open_in_place(nonce, Aad::from(name.as_bytes()), &mut data)
            .map_err(|_| bad())?;
        String::from_utf8(plain.to_vec()).map_err(|_| bad())
    }
}

/// The key from `env_key` (the value of [`KEY_ENV`]) or `<data_dir>/secret.key`, if either exists.
pub fn find(env_key: Option<&str>, data_dir: Option<&Path>) -> Result<Option<SecretKey>, String> {
    if let Some(text) = env_key.filter(|t| !t.trim().is_empty()) {
        return SecretKey::decode(text).map(Some);
    }
    match data_dir.map(|d| d.join(KEY_FILE)) {
        Some(path) if path.is_file() => {
            let text = std::fs::read_to_string(&path)
                .map_err(|e| format!("cannot read `{}`: {e}", path.display()))?;
            SecretKey::decode(&text)
                .map(Some)
                .map_err(|e| format!("`{}`: {e}", path.display()))
        }
        _ => Ok(None),
    }
}

/// The server's key: [`find`], or a new one written to `<data_dir>/secret.key` (owner-only).
pub fn load_or_create(env_key: Option<&str>, data_dir: &Path) -> Result<SecretKey, String> {
    if let Some(key) = find(env_key, Some(data_dir))? {
        return Ok(key);
    }
    let key = SecretKey::generate()?;
    let path = data_dir.join(KEY_FILE);
    write_private(&path, &format!("{}\n", key.encode()))
        .map_err(|e| format!("cannot write `{}`: {e}", path.display()))?;
    Ok(key)
}

#[cfg(unix)]
fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(text.as_bytes())
}

#[cfg(not(unix))]
fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    f.write_all(text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_and_open() {
        let key = SecretKey::generate().unwrap();
        let sealed = key.seal("token", "s3cr3t").unwrap();
        assert!(is_sealed(&sealed));
        assert!(!sealed.contains("s3cr3t"));
        assert_ne!(
            sealed,
            key.seal("token", "s3cr3t").unwrap(),
            "random nonces"
        );
        assert_eq!(key.open("token", &sealed).unwrap(), "s3cr3t");
        // Bound to the name, the key and the bytes.
        assert!(key.open("password", &sealed).is_err());
        assert!(
            SecretKey::generate()
                .unwrap()
                .open("token", &sealed)
                .is_err()
        );
        let mut tampered = sealed.clone();
        tampered.pop();
        assert!(key.open("token", &tampered).is_err());
        assert!(key.open("token", "enc:v1:").is_err());
        assert_eq!(key.open("token", "plain").unwrap(), "plain");
        assert_eq!(format!("{key:?}"), "SecretKey(<redacted>)");
    }

    #[test]
    fn keys_from_the_environment_and_the_data_dir() {
        let key = SecretKey::generate().unwrap();
        let again = SecretKey::decode(&key.encode()).unwrap();
        assert_eq!(again.open("a", &key.seal("a", "v").unwrap()).unwrap(), "v");
        assert!(SecretKey::decode("short").is_err());

        let dir = std::env::temp_dir().join(format!("snapreq-key-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(find(None, Some(&dir)).unwrap().is_none());
        let made = load_or_create(None, &dir).unwrap();
        let found = find(None, Some(&dir)).unwrap().unwrap();
        assert_eq!(found.open("a", &made.seal("a", "v").unwrap()).unwrap(), "v");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join(KEY_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        // The environment wins.
        let env = find(Some(&key.encode()), Some(&dir)).unwrap().unwrap();
        assert!(env.open("a", &made.seal("a", "v").unwrap()).is_err());
    }
}
