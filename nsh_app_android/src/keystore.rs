//! Android Keystore backing for the wallet's secret password.
//!
//! The shared bridge reads `FIBER_SECRET_KEY_PASSWORD` first and falls back
//! to a file next to the data directory. On Android we set that variable from
//! the Android Keystore before the bridge initializes, so the wallet password
//! lives in the hardware-backed keystore: the password is generated on first
//! run, encrypted with a non-exportable AES-GCM key (an alias in
//! `AndroidKeyStore`), and the ciphertext is stored in the app's files
//! directory. The app sandbox already isolates that directory; this keeps
//! the key encryption password out of it in plain form.
//!
//! Everything goes through JNI against the Application context, mirroring
//! the lookup the scanner does (see `scanner.rs` for why there is no other
//! way in). Failures fall back to the bridge's file-based development path
//! rather than blocking startup; the error is logged for diagnosis.

use std::path::PathBuf;

use jni::objects::{GlobalRef, JByteArray, JObject, JString, JValue};
use jni::{JNIEnv, JavaVM};
use tracing::{info, warn};

use crate::scanner::{current_application, java_vm};

const ALIAS: &str = "fiber-wallet-key-password";
const FILE_NAME: &str = "fiber-wallet-key-password.bin";
const IV_LEN: usize = 12;
/// `Cipher.DECRYPT_MODE` / `Cipher.ENCRYPT_MODE`.
const CIPHER_DECRYPT_MODE: i32 = 2;
const CIPHER_ENCRYPT_MODE: i32 = 1;
/// `KeyGenParameterSpec.PURPOSE_ENCRYPT | PURPOSE_DECRYPT`.
const KEY_PURPOSES: i32 = 3;

/// Loads the wallet password from the Keystore, creating one on first run,
/// and exports it as `FIBER_SECRET_KEY_PASSWORD` for the bridge.
///
/// Must run before the first `bridge::WalletBridge::global()` call and
/// before `config::load()`. Android has no `$HOME` sandbox layout for the
/// shared bridge's and config's defaults, so this also points the wallet
/// data directory and the config file at this app's files directory — even
/// when the Keystore path itself fails, so the shell stays usable.
pub fn install_keystore_password() {
    if std::env::var_os("FIBER_SECRET_KEY_PASSWORD").is_some_and(|value| !value.is_empty()) {
        return;
    }

    let Some(vm) = java_vm() else {
        warn!("no JavaVM: wallet data and config stay on the default paths");
        return;
    };
    let mut env = match vm.attach_current_thread() {
        Ok(env) => env,
        Err(error) => {
            warn!(%error, "JNI attach failed: wallet data and config stay on the default paths");
            return;
        }
    };
    let Some(app_object) = current_application(&mut env) else {
        warn!("no application context: wallet data and config stay on the default paths");
        return;
    };
    let Ok(application) = env.new_global_ref(app_object) else {
        warn!("application ref failed: wallet data and config stay on the default paths");
        return;
    };
    let Some(files_dir) = files_dir(&mut env, &application) else {
        warn!("no files directory: wallet data and config stay on the default paths");
        return;
    };

    // Android has no `$HOME` sandbox layout for the shared bridge's and
    // config's defaults, so point both at this app's files directory before
    // anything reads them.
    let data_dir = files_dir.join("fiber-wallet");
    if let Err(error) = std::fs::create_dir_all(&data_dir) {
        warn!(%error, "wallet data directory creation failed");
    }
    nsh_app_fiber_wallet_nsh::bridge::set_wallet_data_directory(data_dir);
    nsh_app_fiber_wallet_nsh::config::set_config_path(files_dir.join("config.json"));

    let path = files_dir.join(FILE_NAME);
    let password = match std::fs::read(&path) {
        Ok(blob) => decrypt(&mut env, &application, &blob).map_err(|error| {
            warn!(%error, "Keystore ciphertext unreadable; generating a fresh password");
            error
        }),
        Err(_) => Err("no stored password yet".to_owned()),
    };
    let Some(password) = password.ok().or_else(|| create_and_store(&mut env, &application, &path))
    else {
        return;
    };

    // Single-threaded engine startup reads this before any other consumer;
    // the process-wide env slot is ours to set. set_var is unsafe under
    // edition 2024 because of multithreaded readers — this runs before the
    // engine starts other threads.
    // SAFETY: called from App() before the engine starts other threads.
    unsafe { std::env::set_var("FIBER_SECRET_KEY_PASSWORD", &password) };
    info!("wallet password loaded from the Android Keystore");
}

fn files_dir(env: &mut JNIEnv<'_>, application: &GlobalRef) -> Option<PathBuf> {
    let dir = env
        .call_method(application.as_obj(), "getFilesDir", "()Ljava/io/File;", &[])
        .ok()?
        .l()
        .ok()?;
    let path = env
        .call_method(&dir, "getAbsolutePath", "()Ljava/lang/String;", &[])
        .ok()?
        .l()
        .ok()?;
    let jstring = JString::from(path);
    let text = env.get_string(&jstring).ok()?;
    Some(PathBuf::from(text.to_string_lossy().into_owned()))
}

/// Decrypts `[iv || ciphertext]` with the keystore key. The key is created on
/// first use and never leaves the keystore.
fn decrypt(env: &mut JNIEnv<'_>, application: &GlobalRef, blob: &[u8]) -> Result<String, String> {
    if blob.len() <= IV_LEN {
        return Err("ciphertext too short".into());
    }
    let key = keystore_key(env, application)?;
    let cipher = cipher(env)?;
    let iv = env
        .byte_array_from_slice(&blob[..IV_LEN])
        .map_err(|e| e.to_string())?;
    let spec = gcm_spec(env, &iv)?;
    init_cipher(env, &cipher, CIPHER_DECRYPT_MODE, &key, Some(&spec))?;
    let ciphertext = env
        .byte_array_from_slice(&blob[IV_LEN..])
        .map_err(|e| e.to_string())?;
    let plain = env
        .call_method(&cipher, "doFinal", "([B)[B", &[JValue::Object(&ciphertext)])
        .map_err(|e| cleared(env, format!("doFinal: {e}")))?
        .l()
        .map_err(|e| e.to_string())?;
    let bytes = env
        .convert_byte_array(JByteArray::from(plain))
        .map_err(|e| e.to_string())?;
    String::from_utf8(bytes).map_err(|_| "password is not UTF-8".to_owned())
}

/// Generates a fresh password, seals it under a new keystore key and writes
/// `[iv || ciphertext]` to `path`.
fn create_and_store(
    env: &mut JNIEnv<'_>,
    application: &GlobalRef,
    path: &std::path::Path,
) -> Option<String> {
    let result = (|| -> Result<String, String> {
        create_keystore_key(env, application)?;
        let key = keystore_key(env, application)?;

        let mut raw = [0_u8; 32];
        getrandom::fill(&mut raw).map_err(|e| format!("random: {e}"))?;
        let password: String = raw.iter().map(|byte| format!("{byte:02x}")).collect();

        // Default provider: the platform reroutes to AndroidKeyStore for its
        // own keys. For GCM encryption the IV is generated by the keystore
        // (passing one throws), so it is read back off the cipher instead.
        let cipher = cipher(env)?;
        init_cipher(env, &cipher, CIPHER_ENCRYPT_MODE, &key, None)?;

        let plain = env
            .byte_array_from_slice(password.as_bytes())
            .map_err(|e| e.to_string())?;
        let sealed = env
            .call_method(&cipher, "doFinal", "([B)[B", &[JValue::Object(&plain)])
            .map_err(|e| cleared(env, format!("doFinal: {e}")))?
            .l()
            .map_err(|e| e.to_string())?;
        let sealed = env
            .convert_byte_array(JByteArray::from(sealed))
            .map_err(|e| e.to_string())?;
        let iv = env
            .call_method(&cipher, "getIV", "()[B", &[])
            .map_err(|e| cleared(env, format!("getIV: {e}")))?
            .l()
            .map_err(|e| e.to_string())?;
        let iv = env
            .convert_byte_array(JByteArray::from(iv))
            .map_err(|e| e.to_string())?;
        if iv.len() != IV_LEN {
            return Err(format!("keystore produced a {}-byte IV", iv.len()));
        }

        let mut blob = Vec::with_capacity(IV_LEN + sealed.len());
        blob.extend_from_slice(&iv);
        blob.extend_from_slice(&sealed);
        std::fs::write(path, blob).map_err(|e| format!("write {path:?}: {e}"))?;
        Ok(password)
    })();
    match result {
        Ok(password) => Some(password),
        Err(error) => {
            warn!(%error, "Keystore password creation failed");
            None
        }
    }
}

/// `GCMParameterSpec(128, iv)` over a 12-byte nonce.
fn gcm_spec<'a>(
    env: &mut JNIEnv<'a>,
    iv: &jni::objects::JByteArray<'_>,
) -> Result<JObject<'a>, String> {
    env.new_object(
        "javax/crypto/spec/GCMParameterSpec",
        "(I[B)V",
        &[JValue::Int((IV_LEN * 8) as i32), JValue::Object(iv)],
    )
    .map_err(|e| cleared(env, format!("GCMParameterSpec: {e}")))
}

/// The `AndroidKeyStore`-backed secret key for [`ALIAS`], or an error when
/// the alias has no key yet.
fn keystore_key<'a>(
    env: &mut JNIEnv<'a>,
    _application: &GlobalRef,
) -> Result<JObject<'a>, String> {
    let store_name = env.new_string("AndroidKeyStore").map_err(|e| e.to_string())?;
    let store = env
        .call_static_method(
            "java/security/KeyStore",
            "getInstance",
            "(Ljava/lang/String;)Ljava/security/KeyStore;",
            &[JValue::Object(&store_name)],
        )
        .map_err(|e| cleared(env, format!("KeyStore.getInstance: {e}")))?
        .l()
        .map_err(|e| e.to_string())?;
    env.call_method(
        &store,
        "load",
        "(Ljava/security/KeyStore$LoadStoreParameter;)V",
        &[JValue::Object(&JObject::null())],
    )
    .map_err(|e| cleared(env, format!("KeyStore.load: {e}")))?;
    let alias = env.new_string(ALIAS).map_err(|e| e.to_string())?;
    let key = env
        .call_method(
            &store,
            "getKey",
            "(Ljava/lang/String;[C)Ljava/security/Key;",
            &[JValue::Object(&alias), JValue::Object(&JObject::null())],
        )
        .map_err(|e| cleared(env, format!("getKey: {e}")))?
        .l()
        .map_err(|e| e.to_string())?;
    if key.is_null() {
        return Err("no keystore key yet".into());
    }
    Ok(key)
}

/// Creates the AES-GCM keystore key (non-exportable, hardware-backed where
/// available). Overwrites any previous key under the alias.
fn create_keystore_key(
    env: &mut JNIEnv<'_>,
    _application: &GlobalRef,
) -> Result<(), String> {
    let alias = env.new_string(ALIAS).map_err(|e| e.to_string())?;
    let builder = env
        .new_object(
            "android/security/keystore/KeyGenParameterSpec$Builder",
            "(Ljava/lang/String;I)V",
            &[JValue::Object(&alias), JValue::Int(KEY_PURPOSES)],
        )
        .map_err(|e| cleared(env, format!("KeyGenParameterSpec.Builder: {e}")))?;

    let gcm = env.new_string("GCM").map_err(|e| e.to_string())?;
    let modes = env
        .new_object_array(1, "java/lang/String", &gcm)
        .map_err(|e| e.to_string())?;
    let no_padding = env.new_string("NoPadding").map_err(|e| e.to_string())?;
    let paddings = env
        .new_object_array(1, "java/lang/String", &no_padding)
        .map_err(|e| e.to_string())?;
    let builder = env
        .call_method(
            &builder,
            "setBlockModes",
            "([Ljava/lang/String;)Landroid/security/keystore/KeyGenParameterSpec$Builder;",
            &[JValue::Object(&modes)],
        )
        .map_err(|e| cleared(env, format!("setBlockModes: {e}")))?
        .l()
        .map_err(|e| e.to_string())?;
    let builder = env
        .call_method(
            &builder,
            "setEncryptionPaddings",
            "([Ljava/lang/String;)Landroid/security/keystore/KeyGenParameterSpec$Builder;",
            &[JValue::Object(&paddings)],
        )
        .map_err(|e| cleared(env, format!("setEncryptionPaddings: {e}")))?
        .l()
        .map_err(|e| e.to_string())?;
    let builder = env
        .call_method(
            &builder,
            "setKeySize",
            "(I)Landroid/security/keystore/KeyGenParameterSpec$Builder;",
            &[JValue::Int(256)],
        )
        .map_err(|e| cleared(env, format!("setKeySize: {e}")))?
        .l()
        .map_err(|e| e.to_string())?;
    let spec = env
        .call_method(
            &builder,
            "build",
            "()Landroid/security/keystore/KeyGenParameterSpec;",
            &[],
        )
        .map_err(|e| cleared(env, format!("build: {e}")))?
        .l()
        .map_err(|e| e.to_string())?;

    let algorithm = env.new_string("AES").map_err(|e| e.to_string())?;
    let provider = env.new_string("AndroidKeyStore").map_err(|e| e.to_string())?;
    let generator = env
        .call_static_method(
            "javax/crypto/KeyGenerator",
            "getInstance",
            "(Ljava/lang/String;Ljava/lang/String;)Ljavax/crypto/KeyGenerator;",
            &[JValue::Object(&algorithm), JValue::Object(&provider)],
        )
        .map_err(|e| cleared(env, format!("KeyGenerator.getInstance: {e}")))?
        .l()
        .map_err(|e| e.to_string())?;
    env.call_method(
        &generator,
        "init",
        "(Ljava/security/spec/AlgorithmParameterSpec;)V",
        &[JValue::Object(&spec)],
    )
    .map_err(|e| cleared(env, format!("KeyGenerator.init: {e}")))?;
    env.call_method(&generator, "generateKey", "()Ljavax/crypto/SecretKey;", &[])
        .map_err(|e| cleared(env, format!("generateKey: {e}")))?;
    Ok(())
}

fn cipher<'a>(env: &mut JNIEnv<'a>) -> Result<JObject<'a>, String> {
    let transform = env.new_string("AES/GCM/NoPadding").map_err(|e| e.to_string())?;
    // No named provider: the default lookup reroutes to AndroidKeyStore for
    // keystore keys; the AndroidKeyStore provider itself registers no
    // "AES/GCM/NoPadding" service.
    env.call_static_method(
        "javax/crypto/Cipher",
        "getInstance",
        "(Ljava/lang/String;)Ljavax/crypto/Cipher;",
        &[JValue::Object(&transform)],
    )
    .map_err(|e| cleared(env, format!("Cipher.getInstance: {e}")))?
    .l()
    .map_err(|e| e.to_string())
}

/// `Cipher.init`; `spec` is required for decryption and must be None for
/// encryption (the keystore generates and stores the IV).
fn init_cipher(
    env: &mut JNIEnv<'_>,
    cipher: &jni::objects::JObject<'_>,
    mode: i32,
    key: &jni::objects::JObject<'_>,
    spec: Option<&jni::objects::JObject<'_>>,
) -> Result<(), String> {
    match spec {
        Some(spec) => {
            env.call_method(
                cipher,
                "init",
                "(ILjava/security/Key;Ljava/security/spec/AlgorithmParameterSpec;)V",
                &[JValue::Int(mode), JValue::Object(key), JValue::Object(spec)],
            )
            .map_err(|e| cleared(env, format!("Cipher.init: {e}")))?;
        }
        None => {
            env.call_method(
                cipher,
                "init",
                "(ILjava/security/Key;)V",
                &[JValue::Int(mode), JValue::Object(key)],
            )
            .map_err(|e| cleared(env, format!("Cipher.init: {e}")))?;
        }
    }
    Ok(())
}

/// Clears a pending Java exception, returning its message for the error log.
pub(crate) fn exception_text(env: &mut JNIEnv<'_>) -> Option<String> {
    let throwable = env.exception_occurred().ok()?;
    if throwable.is_null() {
        return None;
    }
    // Clear first: any further JNI call would itself fail while the
    // exception is pending; the local ref stays valid afterwards.
    let _ = env.exception_clear();
    let text = env
        .call_method(&throwable, "toString", "()Ljava/lang/String;", &[])
        .ok()
        .and_then(|value| value.l().ok())
        .map(JString::from)?;
    let message = env
        .get_string(&text)
        .ok()
        .map(|message| message.to_string_lossy().into_owned());
    message
}

fn cleared(env: &mut JNIEnv<'_>, message: String) -> String {
    if let Some(exception) = exception_text(env) {
        return format!("{message}: {exception}");
    }
    let _ = env.exception_clear();
    message
}

