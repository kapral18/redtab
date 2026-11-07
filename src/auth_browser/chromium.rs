//! macOS Chromium-family cookie import that reads the browser's Keychain key in this
//! process, so the Keychain prompt names redtab and "Always Allow" trusts only redtab.

use std::path::Path;

use aes::cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7};
use anyhow::{Context, Result, anyhow, bail};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use super::BrowserCookie;

const REDDIT_HOSTS: &str = "'reddit.com', '.reddit.com', 'www.reddit.com', '.www.reddit.com'";
/// Chromium cookie databases from this schema on prefix each value with SHA-256(host_key).
const HOST_HASH_VERSION: u32 = 24;
/// Seconds from 1601-01-01 (Chromium's epoch) to 1970-01-01.
const WINDOWS_EPOCH_OFFSET: i64 = 11_644_473_600;

pub(super) struct Keychain {
    pub service: &'static str,
    pub account: &'static str,
}

pub(super) fn keychain(browser: &str) -> Option<Keychain> {
    let (service, account) = match browser {
        "chrome" => ("Chrome Safe Storage", "Chrome"),
        "chromium" => ("Chromium Safe Storage", "Chromium"),
        "brave" => ("Brave Safe Storage", "Brave"),
        "edge" => ("Microsoft Edge Safe Storage", "Microsoft Edge"),
        "arc" => ("Arc Safe Storage", "Arc"),
        "dia" => ("Dia Safe Storage", "Dia"),
        _ => return None,
    };
    Some(Keychain { service, account })
}

pub(super) fn import(database: &Path, keychain: &Keychain) -> Result<Vec<BrowserCookie>> {
    const USER_CANCELED: i32 = -128;
    const AUTH_FAILED: i32 = -25293;
    let password = Zeroizing::new(
        security_framework::passwords::get_generic_password(keychain.service, keychain.account)
            .map_err(|error| match error.code() {
                USER_CANCELED | AUTH_FAILED => anyhow!(
                    "Keychain access to \"{}\" was denied. Allow redtab, or use the separate login window.",
                    keychain.service
                ),
                _ => anyhow!(
                    "Cannot read \"{}\" from the Keychain. Use the separate login window.",
                    keychain.service
                ),
            })?,
    );
    read(database, &derive_key(&password))
}

fn derive_key(password: &[u8]) -> Zeroizing<[u8; 16]> {
    let mut key = Zeroizing::new([0; 16]);
    pbkdf2::pbkdf2_hmac::<sha1::Sha1>(password, b"saltysalt", 1003, key.as_mut());
    key
}

/// Reads a private copy: the browser may hold the database and its journal open.
fn read(database: &Path, key: &[u8; 16]) -> Result<Vec<BrowserCookie>> {
    let copy =
        tempfile::tempdir().context("Cannot create a private copy of the cookie database")?;
    let name = database
        .file_name()
        .and_then(|name| name.to_str())
        .context("Invalid cookie database path")?;
    let target = copy.path().join("Cookies");
    std::fs::copy(database, &target).context("Cannot copy the browser's cookie database")?;
    for suffix in ["-wal", "-journal"] {
        let sidecar = database.with_file_name(format!("{name}{suffix}"));
        if sidecar.is_file() {
            std::fs::copy(&sidecar, copy.path().join(format!("Cookies{suffix}")))
                .context("Cannot copy the browser's cookie journal")?;
        }
    }
    let connection =
        rusqlite::Connection::open(&target).context("Cannot open the cookie database")?;
    let version: u32 = connection
        .query_row(
            "SELECT CAST(value AS TEXT) FROM meta WHERE key = 'version'",
            [],
            |row| row.get::<_, String>(0),
        )
        .context("Cannot read the cookie database version")?
        .parse()
        .context("Invalid cookie database version")?;
    let mut statement = connection.prepare(&format!(
        "SELECT host_key, path, is_secure, expires_utc, name, value, encrypted_value \
         FROM cookies WHERE host_key IN ({REDDIT_HOSTS})"
    ))?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, bool>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, Vec<u8>>(6)?,
        ))
    })?;
    let mut cookies = Vec::new();
    for row in rows {
        let (domain, path, secure, expires, name, value, encrypted) = row?;
        let value = if encrypted.is_empty() {
            value
        } else {
            decrypt(key, &domain, &encrypted, version)?
        };
        cookies.push(BrowserCookie {
            expires: unix_expiry(expires),
            domain,
            path,
            name,
            value,
            secure,
        });
    }
    Ok(cookies)
}

fn decrypt(key: &[u8; 16], host: &str, encrypted: &[u8], version: u32) -> Result<String> {
    let Some(body) = encrypted.strip_prefix(b"v10") else {
        bail!("This browser profile uses an unsupported cookie encryption");
    };
    let mut buffer = Zeroizing::new(body.to_vec());
    let plain = cbc::Decryptor::<aes::Aes128>::new(key.into(), &[b' '; 16].into())
        .decrypt_padded_mut::<Pkcs7>(&mut buffer)
        .map_err(|_| anyhow!("The Keychain key does not decrypt this profile's cookies"))?;
    let plain = if version >= HOST_HASH_VERSION {
        plain
            .strip_prefix(Sha256::digest(host.as_bytes()).as_slice())
            .context("A decrypted cookie does not match its site")?
    } else {
        plain
    };
    String::from_utf8(plain.to_vec()).context("A decrypted cookie is not text")
}

fn unix_expiry(chromium: i64) -> u64 {
    if chromium <= 0 {
        0
    } else {
        u64::try_from(chromium / 1_000_000 - WINDOWS_EPOCH_OFFSET).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
            .collect()
    }

    /// Ciphertext from `openssl enc -aes-128-cbc` with a key from Python's
    /// `hashlib.pbkdf2_hmac('sha1', b'redtab-test-password', b'saltysalt', 1003, 16)`.
    #[test]
    fn chromium_v10_cookies_should_decrypt_with_the_keychain_password() {
        let key = derive_key(b"redtab-test-password");
        assert_eq!(key.as_slice(), hex("df4ba2d7e80c01551a757a93c460192d"));
        let mut encrypted = b"v10".to_vec();
        encrypted.extend(hex(
            "9691b2174245ac96f776c5b5bdb6e6b212f6603a3cdc2b3440ddc58abb16e095f7833b5fcdf3a3e2ee4141cc7bc0c98e",
        ));

        assert_eq!(
            decrypt(&key, ".reddit.com", &encrypted, 24).unwrap(),
            "session-value"
        );
        assert!(decrypt(&key, "www.reddit.com", &encrypted, 24).is_err());
        assert!(decrypt(&derive_key(b"wrong"), ".reddit.com", &encrypted, 24).is_err());
    }
}
