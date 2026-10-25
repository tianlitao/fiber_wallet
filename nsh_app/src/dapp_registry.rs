//! Persistent registry of user-visible dApps.
//!
//! The file is independent of `config.json` and has a versioned schema:
//!
//! ```json
//! {
//!   "schema_version": 1,
//!   "dapps": [
//!     {
//!       "id": "builtin:ckb-explorer",
//!       "name": "CKB 浏览器",
//!       "url": "https://explorer.nervos.org",
//!       "description": "链上交易、单元格与脚本",
//!       "builtin": true
//!     }
//!   ]
//! }
//! ```
//!
//! User dApps are appended to the same file; the built-ins are merged back
//! in when loading so a deleted or malformed built-in cannot remove the
//! supplied sample set. URL policy is enforced here as well as in the UI, so
//! a hand-edited JSON file cannot sneak an unsafe navigation target past the
//! page's own checks.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use url::Url;

/// Current on-disk registry schema.
pub const SCHEMA_VERSION: u32 = 1;

/// The app-only URL of the compiled protocol self-test page. It is never
/// fetched and cannot be supplied by a user dApp.
pub const SELFTEST_URL: &str = "fiber-wallet://protocol-selftest";

/// A dApp the host may open in a WebView.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DappEntry {
    pub id: String,
    pub name: String,
    pub url: String,
    pub description: String,
    /// Built-in entries are supplied by the app and cannot be removed by the
    /// user; user entries have `false`.
    #[serde(default)]
    pub builtin: bool,
    /// Host-controlled HTML for built-in diagnostics. This field is never
    /// accepted from the user registry file: built-ins are replaced by the
    /// compiled set on load.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inline_html: Option<String>,
}

/// The JSON file's root object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DappRegistryFile {
    pub schema_version: u32,
    #[serde(default)]
    pub dapps: Vec<DappEntry>,
}

/// Runtime registry plus its load diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DappRegistry {
    path: PathBuf,
    dapps: Vec<DappEntry>,
    warning: Option<String>,
}

impl DappRegistryFile {
    pub fn builtins() -> Vec<DappEntry> {
        vec![
            DappEntry {
                id: "builtin:protocol-selftest".to_owned(),
                name: "协议自检".to_owned(),
                url: SELFTEST_URL.to_owned(),
                description: "本地验证只读 JSON-RPC、未知方法与拒绝策略".to_owned(),
                builtin: true,
                inline_html: Some(crate::dapp_protocol::protocol_selftest_html()),
            },
            DappEntry {
                id: "builtin:ckb-explorer".to_owned(),
                name: "CKB 浏览器".to_owned(),
                url: "https://explorer.nervos.org".to_owned(),
                description: "链上交易、单元格与脚本".to_owned(),
                builtin: true,
                inline_html: None,
            },
            DappEntry {
                id: "builtin:fiber-site".to_owned(),
                name: "Fiber 官网".to_owned(),
                url: "https://fiber.world".to_owned(),
                description: "Fiber 协议、文档与网络资料".to_owned(),
                builtin: true,
                inline_html: None,
            },
        ]
    }
}

impl DappRegistry {
    /// The registry's disk path, for display and tests.
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn dapps(&self) -> &[DappEntry] {
        &self.dapps
    }

    pub fn warning(&self) -> Option<&str> {
        self.warning.as_deref()
    }

    pub fn get(&self, id: &str) -> Option<&DappEntry> {
        self.dapps.iter().find(|dapp| dapp.id == id)
    }

    /// Loads a registry, merging the shipped built-ins. A missing file is
    /// treated as a fresh registry; malformed or unsafe entries are skipped
    /// and reported through `warning`, never silently executed.
    pub fn load(path: PathBuf) -> Self {
        let path_for_error = path.clone();
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Self {
                    path,
                    dapps: DappRegistryFile::builtins(),
                    warning: None,
                };
            }
            Err(error) => {
                return Self {
                    path,
                    dapps: DappRegistryFile::builtins(),
                    warning: Some(format!("读取 dApp 注册表失败：{error}")),
                };
            }
        };

        let parsed = serde_json::from_str::<DappRegistryFile>(&text);
        let (file, warning) = match parsed {
            Ok(file) => (file, None),
            Err(error) => (
                DappRegistryFile {
                    schema_version: SCHEMA_VERSION,
                    dapps: Vec::new(),
                },
                Some(format!("dApp 注册表 JSON 无效，使用内置列表：{error}")),
            ),
        };
        if file.schema_version != SCHEMA_VERSION {
            let mut registry = DappRegistryFile::default_at(path_for_error);
            registry.warning = Some(format!(
                "不支持的 dApp 注册表 schema_version={}（期望 {SCHEMA_VERSION}）",
                file.schema_version
            ));
            return registry;
        }

        let mut merged = DappRegistryFile::builtins();
        let mut urls = merged
            .iter()
            .filter_map(|dapp| normalize_url(&dapp.url).ok())
            .collect::<HashSet<_>>();
        for entry in file.dapps {
            if entry.builtin {
                continue;
            }
            match validate_entry(&entry) {
                Ok(()) => {
                    let normalized =
                        normalize_url(&entry.url).expect("validate_entry already accepted the URL");
                    if merged.iter().any(|known| known.id == entry.id) || !urls.insert(normalized) {
                        continue;
                    }
                    merged.push(entry);
                }
                Err(error) => {
                    return Self {
                        path: path_for_error,
                        dapps: merged,
                        warning: Some(format!("已忽略无效 dApp：{error}")),
                    };
                }
            }
        }

        Self {
            path,
            dapps: merged,
            warning,
        }
    }

    /// Adds a user dApp and persists the complete registry. IDs are derived
    /// from the normalized URL, making duplicate URLs deterministic.
    pub fn add_user(
        &mut self,
        name: &str,
        url: &str,
        description: &str,
    ) -> Result<&DappEntry, String> {
        let name = name.trim();
        let description = description.trim();
        let url = normalize_url(url)?;
        if name.is_empty() || name.chars().count() > 80 {
            return Err("名称必须为 1-80 个字符".to_owned());
        }
        if description.chars().count() > 240 {
            return Err("描述不能超过 240 个字符".to_owned());
        }
        if self.dapps.iter().any(|dapp| dapp.url == url) {
            return Err("该 URL 已在注册表中".to_owned());
        }
        let id = format!("user:{}", short_hash(&url));
        if self.dapps.iter().any(|dapp| dapp.id == id) {
            return Err("该 dApp id 已存在".to_owned());
        }
        let entry = DappEntry {
            id,
            name: name.to_owned(),
            url,
            description: description.to_owned(),
            builtin: false,
            inline_html: None,
        };
        validate_entry(&entry)?;
        self.dapps.push(entry);
        if let Err(error) = self.persist() {
            self.dapps.pop();
            return Err(error);
        }
        Ok(self.dapps.last().expect("just inserted dApp"))
    }

    pub fn remove_user(&mut self, id: &str) -> Result<(), String> {
        let Some(index) = self.dapps.iter().position(|dapp| dapp.id == id) else {
            return Err("dApp 不存在".to_owned());
        };
        if self.dapps[index].builtin {
            return Err("内置 dApp 不能删除".to_owned());
        }
        let removed = self.dapps.remove(index);
        if let Err(error) = self.persist() {
            self.dapps.insert(index, removed);
            return Err(error);
        }
        Ok(())
    }

    fn persist(&self) -> Result<(), String> {
        persist(&self.path, &self.dapps)
    }
}

impl DappRegistryFile {
    /// A registry backed by `path` with no persisted user entries. Used when
    /// the file is missing or invalid, and by tests.
    pub fn default_at(path: PathBuf) -> DappRegistry {
        DappRegistry {
            path,
            dapps: Self::builtins(),
            warning: None,
        }
    }
}

pub fn default_registry_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_owned());
    PathBuf::from(home).join("Library/Application Support/FiberWalletNSH/dapps.json")
}

/// Persists `dapps` atomically: write a sibling temporary file, then rename
/// it over the registry. The temporary file is created 0600 because a dApp
/// list is user data, not executable authority.
pub fn persist(path: &Path, dapps: &[DappEntry]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("创建 dApp 目录失败：{error}"))?;
    }
    let file = DappRegistryFile {
        schema_version: SCHEMA_VERSION,
        dapps: dapps.iter().filter(|dapp| !dapp.builtin).cloned().collect(),
    };
    let text = serde_json::to_string_pretty(&file)
        .map_err(|error| format!("编码 dApp 注册表失败：{error}"))?;
    let temporary = path.with_extension("json.tmp");
    {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut handle = options
            .open(&temporary)
            .map_err(|error| format!("打开临时 dApp 注册表失败：{error}"))?;
        use std::io::Write;
        handle
            .write_all(text.as_bytes())
            .and_then(|_| handle.sync_all())
            .map_err(|error| format!("写入 dApp 注册表失败：{error}"))?;
    }
    std::fs::rename(&temporary, path).map_err(|error| format!("替换 dApp 注册表失败：{error}"))?;
    Ok(())
}

/// Validates the URL policy and the non-URL fields of an entry.
pub fn validate_entry(entry: &DappEntry) -> Result<(), String> {
    if entry.id.is_empty() || entry.id.chars().any(char::is_control) {
        return Err("dApp id 不能为空或包含控制字符".to_owned());
    }
    if entry.name.trim().is_empty() || entry.name.chars().count() > 80 {
        return Err("dApp 名称必须为 1-80 个字符".to_owned());
    }
    if entry.description.chars().count() > 240 {
        return Err("dApp 描述不能超过 240 个字符".to_owned());
    }
    if entry.inline_html.is_some() {
        if !entry.builtin || entry.url != SELFTEST_URL {
            return Err("用户 dApp 不能提供 inline HTML".to_owned());
        }
        return Ok(());
    }
    normalize_url(&entry.url).map(|_| ())
}

/// Normalizes and validates an entry URL:
/// - HTTPS only;
/// - HTTP only for `localhost`, `127.0.0.1`, or `[::1]` development hosts;
/// - no user-info, fragments, javascript/data/file schemes, or non-web URLs;
/// - no whitespace or control characters.
pub fn normalize_url(input: &str) -> Result<String, String> {
    let input = input.trim();
    if input.is_empty() || input.len() > 2048 {
        return Err("URL 不能为空且不能超过 2048 字节".to_owned());
    }
    if input.chars().any(char::is_whitespace) || input.chars().any(char::is_control) {
        return Err("URL 不能包含空白或控制字符".to_owned());
    }
    let mut url = Url::parse(input).map_err(|error| format!("URL 无效：{error}"))?;
    if url.fragment().is_some() {
        return Err("dApp URL 不能包含 fragment".to_owned());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("URL 不能包含用户信息".to_owned());
    }
    let scheme = url.scheme();
    if !matches!(scheme, "https" | "http") {
        return Err("只允许 https（localhost 开发可使用 http）".to_owned());
    }
    let loopback = url.host().is_some_and(|host| match host {
        url::Host::Domain(host) => is_loopback_host(host),
        url::Host::Ipv4(address) => address.is_loopback(),
        url::Host::Ipv6(address) => address.is_loopback(),
    });
    if scheme == "http" && !loopback {
        return Err("非 https URL 只允许 localhost/127.0.0.1/[::1] 开发地址".to_owned());
    }
    if url.port() == Some(0) {
        return Err("端口无效".to_owned());
    }
    // Elide default ports so equivalent URLs deduplicate identically.
    if (scheme == "https" && url.port() == Some(443))
        || (scheme == "http" && url.port() == Some(80))
    {
        let _ = url.set_port(None);
    }
    Ok(url.to_string())
}

fn is_loopback_host(host: &str) -> bool {
    matches!(
        host.to_ascii_lowercase().as_str(),
        "localhost" | "127.0.0.1" | "::1"
    )
}

fn short_hash(text: &str) -> String {
    // FNV-1a: deterministic, dependency-free ids for local persistence. This
    // is not a security hash and is not used for authentication.
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

impl Default for DappRegistryFile {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            dapps: Self::builtins(),
        }
    }
}

/// A convenience the UI uses for warning display.
pub fn warning_or_empty(registry: &DappRegistry) -> String {
    registry.warning().unwrap_or_default().to_owned()
}

/// Ensures a loaded registry has no duplicate ids or URLs. This is used by
/// tests and by callers that construct a registry programmatically.
pub fn validate_registry(dapps: &[DappEntry]) -> Result<(), String> {
    let mut ids = HashSet::new();
    let mut urls = HashSet::new();
    for dapp in dapps {
        validate_entry(dapp)?;
        if !ids.insert(&dapp.id) {
            return Err(format!("重复 dApp id：{}", dapp.id));
        }
        if dapp.inline_html.is_some() {
            continue;
        }
        let normalized = normalize_url(&dapp.url)?;
        if !urls.insert(normalized) {
            return Err(format!("重复 dApp URL：{}", dapp.url));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "fiber-wallet-dapp-test-{name}-{}-{}",
            std::process::id(),
            short_hash(name)
        ))
    }

    #[test]
    fn accepts_secure_and_loopback_urls() {
        assert_eq!(
            normalize_url("https://example.com/path?q=1").unwrap(),
            "https://example.com/path?q=1"
        );
        assert_eq!(
            normalize_url("http://localhost:3000/").unwrap(),
            "http://localhost:3000/"
        );
        assert_eq!(
            normalize_url("http://127.0.0.1:4173").unwrap(),
            "http://127.0.0.1:4173/"
        );
        assert_eq!(
            normalize_url("http://[::1]:8080/").unwrap(),
            "http://[::1]:8080/"
        );
    }

    #[test]
    fn rejects_unsafe_schemes_and_remote_http() {
        for url in [
            "http://example.com",
            "javascript:alert(1)",
            "data:text/html,hello",
            "file:///etc/passwd",
            "https://user@example.com/",
            "https://example.com/#fragment",
            "https://example.com/a b",
            "ftp://example.com",
        ] {
            assert!(normalize_url(url).is_err(), "{url} should be rejected");
        }
    }

    #[test]
    fn registry_deduplicates_case_insensitive_hosts_and_userinfo_is_rejected() {
        let mut registry = DappRegistryFile::default_at(temp_dir("urls"));
        registry
            .add_user("Example", "https://Example.com/", "")
            .unwrap();
        assert!(
            registry
                .add_user("Again", "https://example.com/", "")
                .unwrap_err()
                .contains("已在")
        );
        assert!(
            registry
                .add_user("Bad", "https://user@example.com/", "")
                .is_err()
        );
    }

    #[test]
    fn persists_user_dapps_and_merges_builtins() {
        let dir = temp_dir("persist");
        let path = dir.join("dapps.json");
        let mut registry = DappRegistry::load(path.clone());
        assert_eq!(registry.dapps().len(), DappRegistryFile::builtins().len());
        registry
            .add_user("Local", "http://localhost:5173", "dev")
            .unwrap();
        let loaded = DappRegistry::load(path);
        assert!(loaded.warning().is_none());
        assert!(loaded.dapps().iter().any(|dapp| dapp.name == "Local"));
        assert!(loaded.get("builtin:ckb-explorer").is_some());
        assert!(validate_registry(loaded.dapps()).is_ok());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn malformed_registry_falls_back_to_builtins() {
        let dir = temp_dir("malformed");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("dapps.json");
        std::fs::write(&path, "{not json").unwrap();
        let registry = DappRegistry::load(path);
        assert!(registry.warning().is_some());
        assert_eq!(registry.dapps().len(), DappRegistryFile::builtins().len());
        let _ = std::fs::remove_dir_all(dir);
    }
}
