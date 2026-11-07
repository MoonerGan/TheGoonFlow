//! Crypto core for The Goon Flow vault.
//!
//! A vault is a plain directory containing:
//! - `vault.marker` — plain marker identifying a goonflow vault (no secrets)
//! - `vault.key`    — keyfile: Argon2id KDF params + XChaCha20-Poly1305-wrapped data key
//! - `vault.db`     — SQLCipher database keyed with the data key
//!
//! Unlock: passphrase → Argon2id master key → unwraps the random data key →
//! `PRAGMA key` on the DB. Wrong passphrase fails on the AEAD tag (no oracle).
//! Lock: drop the data key (zeroized) and close the DB.
//! Nuke: destroy key material, then overwrite every file in the vault dir and
//! delete it (crypto-erase — contents are already ciphertext, one pass is enough).

use argon2::{Algorithm, Argon2, Params, Version};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use chacha20poly1305::aead::generic_array::GenericArray;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::XChaCha20Poly1305;
use rand::rngs::OsRng;
use rand::RngCore;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use zeroize::{Zeroize, ZeroizeOnDrop};

pub const MARKER_FILE: &str = "vault.marker";
pub const KEY_FILE: &str = "vault.key";
pub const DB_FILE: &str = "vault.db";
pub const MARKER_TEXT: &str = "goonflow-vault v1\n";

/// Domain separator for the keyfile AEAD; also pins the format version.
const AAD: &[u8] = b"goonflow-vault-key-v1";
const SALT_LEN: usize = 32;
const NONCE_LEN: usize = 24;

// Argon2id parameters: ~64 MiB, 3 passes, 1 lane. Deliberate cost for a
// human-passphrase KDF; still comfortable on modest hardware.
const KDF_M_KIB: u32 = 64 * 1024;
const KDF_T_ITERS: u32 = 3;
const KDF_P_LANES: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    #[error("invalid passphrase")]
    InvalidPassphrase,
    #[error("not a goonflow vault: {0}")]
    NotAVault(PathBuf),
    #[error("no keyfile in vault: {0}")]
    NoKeyfile(PathBuf),
    #[error("vault already exists: {0}")]
    AlreadyAVault(PathBuf),
    #[error("key derivation failed: {0}")]
    Kdf(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("keyfile malformed: {0}")]
    Keyfile(String),
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
}

/// The key that decrypts the vault database. Zeroized when dropped.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct DataKey(pub [u8; 32]);

/// True if `dir` looks like a vault directory.
pub fn is_vault_dir(dir: &Path) -> bool {
    dir.join(MARKER_FILE).is_file()
}

/// Create vault scaffolding (marker + fresh keyfile) in `dir`.
/// Returns the new data key so the caller can open the DB immediately.
pub fn create_keyfile(dir: &Path, passphrase: &str) -> Result<DataKey, VaultError> {
    if passphrase.is_empty() {
        return Err(VaultError::Keyfile("passphrase must not be empty".into()));
    }
    fs::create_dir_all(dir)?;
    if is_vault_dir(dir) {
        return Err(VaultError::AlreadyAVault(dir.to_path_buf()));
    }
    fs::write(dir.join(MARKER_FILE), MARKER_TEXT)?;

    let (key, record) = build_keyfile(passphrase)?;
    fs::write(dir.join(KEY_FILE), serde_json::to_vec_pretty(&record)?)?;
    Ok(key)
}

/// Unwrap the vault's data key with `passphrase`.
pub fn open_keyfile(dir: &Path, passphrase: &str) -> Result<DataKey, VaultError> {
    if !is_vault_dir(dir) {
        return Err(VaultError::NotAVault(dir.to_path_buf()));
    }
    let path = dir.join(KEY_FILE);
    if !path.is_file() {
        return Err(VaultError::NoKeyfile(path));
    }
    let record: KeyFileRecord = serde_json::from_slice(&fs::read(&path)?)
        .map_err(|e| VaultError::Keyfile(e.to_string()))?;
    unwrap_data_key(passphrase, &record)
}

/// Change the passphrase: unwraps with the old one, re-wraps under a fresh
/// salt + master key. The data key (and thus the DB) never changes.
pub fn change_passphrase(dir: &Path, old: &str, new: &str) -> Result<(), VaultError> {
    if new.is_empty() {
        return Err(VaultError::Keyfile("passphrase must not be empty".into()));
    }
    let key = open_keyfile(dir, old)?;
    let mut record = read_record(dir)?;
    let (salt, master) = derive_master(new)?;
    let wrapped = wrap(&master, &key.0)?;
    record.kdf.salt = B64.encode(&salt);
    record.wrapped = wrapped;
    fs::write(dir.join(KEY_FILE), serde_json::to_vec_pretty(&record)?)?;
    Ok(())
}

/// Open (creating if absent) the SQLCipher DB with the data key.
pub fn open_db(dir: &Path, key: &DataKey) -> Result<Connection, VaultError> {
    let conn = Connection::open(dir.join(DB_FILE))?;
    let hex: String = key.0.iter().map(|b| format!("{b:02x}")).collect();
    let mut pragma = format!("PRAGMA key = \"x'{hex}'\";");
    // Run the PRAGMA first, THEN wipe the secret material — order matters.
    let applied = conn.execute_batch(&pragma);
    pragma.zeroize();
    let mut hex = hex;
    hex.zeroize();
    applied?;
    conn.execute_batch("PRAGMA cipher_memory_security = ON; PRAGMA foreign_keys = ON;")?;
    // Force decryption of the header — a wrong key surfaces here. This DB is
    // always created/managed by us, so *any* SQLite failure at this point
    // means the key is wrong (or the vault is corrupt, same outcome for the
    // user: they need the right key or a backup).
    if let Err(e) = conn.query_row("SELECT count(*) FROM sqlite_master", [], |_| Ok(())) {
        let _ = e;
        return Err(VaultError::InvalidPassphrase);
    }
    Ok(conn)
}

/// Crypto-erase + shred: overwrite every file in the vault dir with random
/// bytes, then delete it. Key material goes first so the remainder is
/// undecryptable even if shredding is interrupted.
pub fn nuke(dir: &Path) -> Result<(), VaultError> {
    if !dir.exists() {
        return Ok(());
    }
    // Keyfile first, deterministically.
    let keyfile = dir.join(KEY_FILE);
    if keyfile.is_file() {
        shred_file(&keyfile)?;
    }
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_file() {
            shred_file(&path)?;
        }
    }
    Ok(())
}

fn shred_file(path: &Path) -> Result<(), VaultError> {
    let len = fs::metadata(path).map(|m| m.len() as usize).unwrap_or(0);
    if len > 0 {
        let mut buf = vec![0u8; len];
        OsRng.fill_bytes(&mut buf);
        if let Ok(mut f) = fs::OpenOptions::new().write(true).open(path) {
            let _ = f.write_all(&buf);
            let _ = f.flush();
            let _ = f.sync_all();
        }
        buf.zeroize();
    }
    fs::remove_file(path)?;
    Ok(())
}

// ---------- keyfile internals ----------

#[derive(Serialize, Deserialize, Clone)]
struct KeyFileRecord {
    version: u32,
    kdf: KdfRecord,
    wrapped: WrappedRecord,
}

#[derive(Serialize, Deserialize, Clone)]
struct KdfRecord {
    alg: String,
    salt: String,
    m_kib: u32,
    t_iters: u32,
    p_lanes: u32,
}

#[derive(Serialize, Deserialize, Clone)]
struct WrappedRecord {
    alg: String,
    nonce: String,
    ct: String,
}

fn read_record(dir: &Path) -> Result<KeyFileRecord, VaultError> {
    let path = dir.join(KEY_FILE);
    if !path.is_file() {
        return Err(VaultError::NoKeyfile(path));
    }
    serde_json::from_slice(&fs::read(path)?).map_err(|e| VaultError::Keyfile(e.to_string()))
}

fn derive_master(passphrase: &str) -> Result<(Vec<u8>, [u8; 32]), VaultError> {
    let mut salt = vec![0u8; SALT_LEN];
    OsRng.fill_bytes(&mut salt);
    let master = derive_master_with(passphrase, &salt)?;
    Ok((salt, master))
}

fn derive_master_with(passphrase: &str, salt: &[u8]) -> Result<[u8; 32], VaultError> {
    let params = Params::new(KDF_M_KIB, KDF_T_ITERS, KDF_P_LANES, Some(32))
        .map_err(|e| VaultError::Kdf(e.to_string()))?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut master = [0u8; 32];
    argon
        .hash_password_into(passphrase.as_bytes(), salt, &mut master)
        .map_err(|e| VaultError::Kdf(e.to_string()))?;
    Ok(master)
}

fn wrap(master: &[u8; 32], data_key: &[u8; 32]) -> Result<WrappedRecord, VaultError> {
    let cipher = XChaCha20Poly1305::new(GenericArray::from_slice(master));
    let mut nonce_bytes = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = GenericArray::from_slice(&nonce_bytes);
    let ct = cipher
        .encrypt(nonce, Payload { aad: AAD, msg: data_key })
        .map_err(|e| VaultError::Keyfile(format!("wrap failed: {e}")))?;
    Ok(WrappedRecord {
        alg: "xchacha20poly1305".into(),
        nonce: B64.encode(nonce_bytes),
        ct: B64.encode(ct),
    })
}

fn unwrap_data_key(passphrase: &str, record: &KeyFileRecord) -> Result<DataKey, VaultError> {
    if record.version != 1 || record.kdf.alg != "argon2id" || record.wrapped.alg != "xchacha20poly1305" {
        return Err(VaultError::Keyfile(format!(
            "unsupported keyfile format v{}",
            record.version
        )));
    }
    let salt = B64
        .decode(&record.kdf.salt)
        .map_err(|e| VaultError::Keyfile(e.to_string()))?;
    let master = derive_master_with(passphrase, &salt)?;
    let nonce_bytes = B64
        .decode(&record.wrapped.nonce)
        .map_err(|e| VaultError::Keyfile(e.to_string()))?;
    if nonce_bytes.len() != NONCE_LEN {
        return Err(VaultError::Keyfile("bad nonce length".into()));
    }
    let ct = B64
        .decode(&record.wrapped.ct)
        .map_err(|e| VaultError::Keyfile(e.to_string()))?;
    let cipher = XChaCha20Poly1305::new(GenericArray::from_slice(&master));
    let plaintext = cipher
        .decrypt(
            GenericArray::from_slice(&nonce_bytes),
            Payload { aad: AAD, msg: &ct },
        )
        .map_err(|_| VaultError::InvalidPassphrase)?;
    if plaintext.len() != 32 {
        return Err(VaultError::Keyfile("bad data key length".into()));
    }
    let mut key_bytes = [0u8; 32];
    key_bytes.copy_from_slice(&plaintext);
    Ok(DataKey(key_bytes))
}

fn build_keyfile(passphrase: &str) -> Result<(DataKey, KeyFileRecord), VaultError> {
    let mut key_bytes = [0u8; 32];
    OsRng.fill_bytes(&mut key_bytes);
    let (salt, master) = derive_master(passphrase)?;
    let wrapped = wrap(&master, &key_bytes)?;
    let record = KeyFileRecord {
        version: 1,
        kdf: KdfRecord {
            alg: "argon2id".into(),
            salt: B64.encode(&salt),
            m_kib: KDF_M_KIB,
            t_iters: KDF_T_ITERS,
            p_lanes: KDF_P_LANES,
        },
        wrapped,
    };
    Ok((DataKey(key_bytes), record))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let mut suffix = String::new();
        let mut raw = [0u8; 8];
        OsRng.fill_bytes(&mut raw);
        for b in raw {
            suffix.push_str(&format!("{b:02x}"));
        }
        let p = std::env::temp_dir().join(format!("goonvault-test-{tag}-{suffix}"));
        let _ = fs::remove_dir_all(&p);
        p
    }

    #[test]
    fn keyfile_roundtrip_and_wrong_passphrase() {
        let dir = temp_dir("roundtrip");
        let key = create_keyfile(&dir, "correct horse battery staple").unwrap();
        assert!(is_vault_dir(&dir));
        let reopened = open_keyfile(&dir, "correct horse battery staple").unwrap();
        assert_eq!(key.0, reopened.0);
        assert!(matches!(
            open_keyfile(&dir, "wrong"),
            Err(VaultError::InvalidPassphrase)
        ));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn db_open_with_key_and_wrong_key_detected() {
        let dir = temp_dir("db");
        let key = create_keyfile(&dir, "pw").unwrap();
        {
            let conn = open_db(&dir, &key).unwrap();
            conn.execute_batch("CREATE TABLE t(x);").unwrap();
        }
        // Re-open with the *wrong* key must fail as InvalidPassphrase, not corrupt silently.
        let wrong = DataKey([7u8; 32]);
        assert!(matches!(open_db(&dir, &wrong), Err(VaultError::InvalidPassphrase)));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn change_passphrase_keeps_data_key() {
        let dir = temp_dir("chpw");
        let original = create_keyfile(&dir, "old").unwrap();
        change_passphrase(&dir, "old", "new").unwrap();
        let after = open_keyfile(&dir, "new").unwrap();
        assert_eq!(original.0, after.0);
        assert!(matches!(open_keyfile(&dir, "old"), Err(VaultError::InvalidPassphrase)));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn nuke_removes_everything() {
        let dir = temp_dir("nuke");
        let key = create_keyfile(&dir, "pw").unwrap();
        let conn = open_db(&dir, &key).unwrap();
        drop(conn);
        nuke(&dir).unwrap();
        for f in [MARKER_FILE, KEY_FILE, DB_FILE] {
            assert!(!dir.join(f).exists(), "{f} survived nuke");
        }
        fs::remove_dir_all(&dir).ok();
    }
}
