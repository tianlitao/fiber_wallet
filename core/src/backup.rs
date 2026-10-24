//! Export and import of the whole wallet state (keystore + channel store) as
//! a single password-encrypted archive.
//!
//! The archive is a gzip-compressed tar encrypted with the same format the
//! node uses for its key files (scrypt + AES, via FNN's
//! `encrypt_decrypt_file`).
//!
//! Semantics are MOVE only: an imported channel state shares the node
//! identity of the instance it was exported from, and running both instances
//! would present one identity with two ledgers to the channel peers — a
//! revocation-punishment hazard. Hosts must make sure the exporting instance
//! is permanently stopped before importing, and only while the local node is
//! stopped (the caller owns both checks).

use std::io::Read;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use fnn::utils::encrypt_decrypt_file::{decrypt_from_file, encrypt_to_file};
use tar::{Archive, Builder, Header};

use crate::types::WalletBackupManifest;

const FORMAT: u32 = 1;
const MANIFEST_NAME: &str = "manifest.json";
const KEYSTORE_FILES: [&str; 2] = ["key", "sk"];
const STORE_DIR: &str = "store";
const MAX_FILE_DEPTH: u8 = 8;

/// Exports the keystore and channel store into `out_path`, encrypted with
/// `password` (at least 8 characters). Requires the node to be stopped: the
/// caller owns that check.
pub fn export_wallet_state(
    data_dir: &Path,
    out_path: &Path,
    password: &str,
    node_id: Option<&str>,
) -> Result<WalletBackupManifest, String> {
    if password.chars().count() < 8 {
        return Err("导出密码至少需要 8 个字符".to_owned());
    }
    let files = collect_files(data_dir)?;

    let manifest = WalletBackupManifest {
        format: FORMAT,
        network: "testnet".to_owned(),
        node_id: node_id.map(str::to_owned),
        created_at_ms: unix_ms(),
    };
    let manifest_bytes =
        serde_json::to_vec_pretty(&manifest).map_err(|err| format!("编码 manifest 失败: {err}"))?;

    let encoder = GzEncoder::new(Vec::new(), Compression::default());
    let mut tar = Builder::new(encoder);
    append_entry(&mut tar, MANIFEST_NAME, &manifest_bytes)?;
    for (name, data) in &files {
        append_entry(&mut tar, name, data)?;
    }
    let archive = tar
        .into_inner()
        .map_err(|err| format!("打包归档失败: {err}"))?
        .finish()
        .map_err(|err| format!("压缩归档失败: {err}"))?;

    encrypt_to_file(out_path, &archive, password.as_bytes())
        .map_err(|err| format!("加密导出文件失败: {err}"))?;
    Ok(manifest)
}

/// Imports an archive produced by [`export_wallet_state`] into `data_dir`,
/// replacing the current keystore and channel store. The replaced state is
/// moved to `import-backup-<unix_ms>/` inside the data directory first, so a
/// failed import can be recovered by hand. Requires the node to be stopped.
pub fn import_wallet_state(
    data_dir: &Path,
    archive_path: &Path,
    password: &str,
) -> Result<WalletBackupManifest, String> {
    let archive = decrypt_from_file(archive_path, password.as_bytes())
        .map_err(|_| "解密失败：密码错误或文件损坏".to_owned())?;

    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    let mut manifest = None;
    for entry in Archive::new(GzDecoder::new(archive.as_slice()))
        .entries()
        .map_err(|err| format!("读取归档失败: {err}"))?
    {
        let mut entry = entry.map_err(|err| format!("读取归档条目失败: {err}"))?;
        let name = entry
            .path()
            .map_err(|err| format!("读取归档路径失败: {err}"))?
            .to_string_lossy()
            .into_owned();
        if name.starts_with('/') || name.split('/').any(|part| part == "..") {
            return Err(format!("归档包含非法路径: {name}"));
        }
        let mut data = Vec::new();
        entry
            .read_to_end(&mut data)
            .map_err(|err| format!("读取归档数据失败: {err}"))?;
        if name == MANIFEST_NAME {
            let parsed: WalletBackupManifest = serde_json::from_slice(&data)
                .map_err(|err| format!("manifest.json 无法解析: {err}"))?;
            manifest = Some(parsed);
        } else if !data.is_empty() || !name.ends_with('/') {
            entries.push((name, data));
        }
    }

    let manifest = manifest.ok_or("归档缺少 manifest.json")?;
    if manifest.format != FORMAT {
        return Err(format!("不支持的导出格式版本 {}", manifest.format));
    }
    if manifest.network != "testnet" {
        return Err(format!(
            "导出来自 {} 网络，本应用仅支持 testnet",
            manifest.network
        ));
    }
    for name in KEYSTORE_FILES {
        if !entries.iter().any(|(path, _)| path == name) {
            return Err(format!("归档缺少 {name}（不是完整的钱包导出）"));
        }
    }
    if !entries.iter().any(|(path, _)| path.starts_with(STORE_DIR)) {
        return Err("归档缺少 store/（不是完整的钱包导出）".to_owned());
    }

    // Move the current state aside before writing anything.
    let backup_dir = data_dir.join(format!("import-backup-{}", unix_ms()));
    std::fs::create_dir_all(&backup_dir).map_err(|err| format!("创建备份目录失败: {err}"))?;
    let names: Vec<&str> = KEYSTORE_FILES.iter().copied().chain([STORE_DIR]).collect();
    for name in names {
        let path = data_dir.join(name);
        if path.exists() {
            std::fs::rename(path, backup_dir.join(name))
                .map_err(|err| format!("备份现有 {name} 失败: {err}"))?;
        }
    }

    for (name, data) in &entries {
        let dest = data_dir.join(name);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|err| format!("创建目录失败: {err}"))?;
        }
        std::fs::write(&dest, data).map_err(|err| format!("写入 {name} 失败: {err}"))?;
    }
    Ok(manifest)
}

/// Every file of the export: the two keystore files plus everything under
/// `store/`.
fn collect_files(data_dir: &Path) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut files = Vec::new();
    for name in KEYSTORE_FILES {
        let path = data_dir.join(name);
        if !path.exists() {
            return Err(format!("数据目录缺少 {name}，不是可导出的钱包状态"));
        }
        let data = std::fs::read(&path).map_err(|err| format!("读取 {name} 失败: {err}"))?;
        files.push((name.to_owned(), data));
    }
    append_dir_recursive(&data_dir.join(STORE_DIR), STORE_DIR, &mut files, 0)?;
    Ok(files)
}

fn append_dir_recursive(
    dir: &Path,
    prefix: &str,
    files: &mut Vec<(String, Vec<u8>)>,
    depth: u8,
) -> Result<(), String> {
    if depth > MAX_FILE_DEPTH {
        return Err("store 目录嵌套过深".to_owned());
    }
    let entries =
        std::fs::read_dir(dir).map_err(|err| format!("读取 {} 失败: {err}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|err| format!("读取目录项失败: {err}"))?;
        let relative = format!("{prefix}/{}", entry.file_name().to_string_lossy());
        let file_type = entry
            .file_type()
            .map_err(|err| format!("读取文件类型失败: {err}"))?;
        if file_type.is_dir() {
            append_dir_recursive(&entry.path(), &relative, files, depth + 1)?;
        } else if file_type.is_file() {
            let data = std::fs::read(entry.path())
                .map_err(|err| format!("读取 {} 失败: {err}", relative))?;
            files.push((relative, data));
        }
    }
    Ok(())
}

fn append_entry(
    tar: &mut Builder<GzEncoder<Vec<u8>>>,
    name: &str,
    data: &[u8],
) -> Result<(), String> {
    let mut header = Header::new_gnu();
    header.set_size(data.len() as u64);
    header.set_mode(0o600);
    header.set_cksum();
    tar.append_data(&mut header, name, data)
        .map_err(|err| format!("写入归档条目 {name} 失败: {err}"))
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_millis() as u64)
        .unwrap_or(0)
}
