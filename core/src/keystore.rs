//! Encrypted keystore maintenance: changing the password that protects the
//! CKB wallet key (`key`) and the node identity key (`sk`).
//!
//! The files are only read when the node starts, so hosts should call this
//! only while the node is stopped; the caller owns that check.

use std::path::Path;

use fnn::utils::encrypt_decrypt_file::{decrypt_from_file, encrypt_to_file};

const KEYSTORE_FILES: [&str; 2] = ["key", "sk"];

/// Re-encrypts every keystore file under `data_dir` from `old_password` to
/// `new_password`. Nothing is written unless every existing file decrypts
/// under the old password first, so a wrong password never corrupts state.
pub fn change_keystore_password(
    data_dir: &Path,
    old_password: &str,
    new_password: &str,
) -> Result<(), String> {
    if new_password.is_empty() {
        return Err("新密码不能为空".to_owned());
    }
    if old_password == new_password {
        return Err("新密码与当前密码相同".to_owned());
    }

    let mut plains = Vec::new();
    for name in KEYSTORE_FILES {
        let path = data_dir.join(name);
        if !path.exists() {
            continue;
        }
        let plain = decrypt_from_file(&path, old_password.as_bytes())
            .map_err(|_| format!("{name} 无法用当前密码解密，未做任何修改"))?;
        plains.push((path, plain));
    }
    if plains.is_empty() {
        return Err("数据目录下没有可重加密的密钥文件".to_owned());
    }

    for (path, plain) in plains {
        encrypt_to_file(&path, &plain, new_password.as_bytes())
            .map_err(|err| format!("重加密 {} 失败: {err}", path.display()))?;
    }
    Ok(())
}
