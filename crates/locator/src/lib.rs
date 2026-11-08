//! Vault location for The Goon Flow.
//!
//! The exe lives anywhere (USB stick, folder on D:, etc). Beside it we keep a
//! tiny plaintext `flow.cfg` pointer recording the vault's *volume label* and
//! path-on-volume, so a vault on removable media is still found after Windows
//! reshuffles drive letters. Nothing sensitive: no passphrase, no content hints.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub const POINTER_FILE: &str = "flow.cfg";
pub const VAULT_MARKER: &str = "vault.marker";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pointer {
    pub version: u32,
    /// Volume label of the drive hosting the vault. Primary identity: survives
    /// drive-letter changes. `None` for unlabeled volumes.
    pub volume_label: Option<String>,
    /// Drive letter hint (e.g. `"D:"`), tried first when still accurate.
    pub drive_hint: Option<String>,
    /// Path from the volume root to the vault dir, e.g. `vaults\\goonflow`.
    pub path_on_volume: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Volume {
    pub letter: char,
    pub label: Option<String>,
    pub root: String,
    pub removable: bool,
}

pub fn pointer_path(exe_dir: &Path) -> PathBuf {
    exe_dir.join(POINTER_FILE)
}

pub fn load(exe_dir: &Path) -> Option<Pointer> {
    let raw = fs::read(pointer_path(exe_dir)).ok()?;
    serde_json::from_slice(&raw).ok()
}

pub fn save(exe_dir: &Path, pointer: &Pointer) -> std::io::Result<()> {
    fs::write(
        pointer_path(exe_dir),
        serde_json::to_vec_pretty(pointer).unwrap(),
    )
}

pub fn clear(exe_dir: &Path) {
    let _ = fs::remove_file(pointer_path(exe_dir));
}

/// Build a pointer from an absolute vault path on a real volume.
pub fn make_pointer(vault_path: &Path) -> Option<Pointer> {
    let raw = vault_path.to_string_lossy().to_string();
    // Tolerate Windows verbatim paths (\\?\C:\...).
    let s = raw.strip_prefix(r"\\?\").unwrap_or(&raw).to_string();
    let drive: String = s.chars().take(2).collect(); // "X:"
    let rest = s.get(3..).unwrap_or("").to_string(); // after "X:\"
    if drive.len() != 2 || !drive.ends_with(':') {
        return None; // UNC / unusual path: not supported for M0
    }
    let volume = volumes()
        .into_iter()
        .find(|v| v.letter == drive.chars().next().unwrap())?;
    Some(Pointer {
        version: 1,
        volume_label: volume.label.clone(),
        drive_hint: Some(drive),
        path_on_volume: rest,
    })
}

/// Resolve a pointer to an existing vault directory, if present right now.
/// Order: drive hint first, then any volume with a matching label.
pub fn resolve(pointer: &Pointer) -> Option<PathBuf> {
    let vols = volumes();

    if let Some(hint) = &pointer.drive_hint {
        let letter = hint.chars().next()?;
        if let Some(v) = vols.iter().find(|v| v.letter == letter) {
            if let Some(p) = candidate(v, pointer) {
                return Some(p);
            }
        }
    }

    if let Some(label) = &pointer.volume_label {
        for v in vols.iter().filter(|v| {
            v.label
                .as_ref()
                .is_some_and(|l| l.eq_ignore_ascii_case(label))
        }) {
            if let Some(p) = candidate(v, pointer) {
                return Some(p);
            }
        }
    }

    None
}

fn candidate(volume: &Volume, pointer: &Pointer) -> Option<PathBuf> {
    let path = Path::new(&volume.root).join(&pointer.path_on_volume);
    if path.join(VAULT_MARKER).is_file() {
        Some(path)
    } else {
        None
    }
}

#[cfg(windows)]
pub fn volumes() -> Vec<Volume> {
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{
        GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW,
    };

    // DRIVE_TYPE newtype values: NO_ROOT_DIR=1, REMOVABLE=2, FIXED=3, REMOTE=4...
    const DRIVE_NO_ROOT_DIR: u32 = 1;
    const DRIVE_REMOVABLE: u32 = 2;

    let mut out = Vec::new();
    let mask = unsafe { GetLogicalDrives() };
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let letter = (b'A' + i as u8) as char;
        let root = format!("{letter}:\\");
        let wroot: Vec<u16> = root.encode_utf16().chain(std::iter::once(0)).collect();

        let drive_type = unsafe { GetDriveTypeW(PCWSTR(wroot.as_ptr())) };
        if drive_type == DRIVE_NO_ROOT_DIR {
            continue;
        }

        let mut name_buf = [0u16; 262];
        let mut fs_buf = [0u16; 262];
        let ok = unsafe {
            GetVolumeInformationW(
                PCWSTR(wroot.as_ptr()),
                Some(&mut name_buf),
                None,
                None,
                None,
                Some(&mut fs_buf),
            )
        }
        .is_ok();

        let label = if ok {
            let end = name_buf
                .iter()
                .position(|c| *c == 0)
                .unwrap_or(name_buf.len());
            let s = String::from_utf16_lossy(&name_buf[..end]);
            if s.is_empty() {
                None
            } else {
                Some(s)
            }
        } else {
            None
        };

        out.push(Volume {
            letter,
            label,
            root,
            removable: drive_type == DRIVE_REMOVABLE,
        });
    }
    out
}

#[cfg(not(windows))]
pub fn volumes() -> Vec<Volume> {
    // Windows-only for now; the desktop target is Windows.
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn pointer_roundtrip_on_temp_vault() {
        // Build a fake vault in a temp dir (on some real volume), then resolve it.
        let dir = std::env::temp_dir().join("goonloc-test-vault");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(VAULT_MARKER), "goonflow-vault v1\n").unwrap();

        let pointer = make_pointer(&dir).expect("pointer for temp path");
        let resolved = resolve(&pointer);
        assert_eq!(resolved.as_deref(), Some(dir.as_path()));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn save_load_clear_around_exe_dir() {
        let exe_dir = std::env::temp_dir().join("goonloc-test-exe");
        fs::create_dir_all(&exe_dir).unwrap();
        let p = Pointer {
            version: 1,
            volume_label: Some("GOONBUS".into()),
            drive_hint: Some("Z:".into()),
            path_on_volume: "flow\\vault".into(),
        };
        save(&exe_dir, &p).unwrap();
        assert_eq!(load(&exe_dir), Some(p));
        clear(&exe_dir);
        assert_eq!(load(&exe_dir), None);
        fs::remove_dir_all(&exe_dir).ok();
    }
}
