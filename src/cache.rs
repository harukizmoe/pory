//! 翻译结果缓存。
//!
//! 为什么要缓存？两个实在的理由：
//! 1. **省免费额度** —— MyMemory 每天只有 1000 次，重复内容不该重复消耗
//! 2. **快** —— 同一句话第二次翻译是本地读取，毫秒级
//!
//! 设计取舍：
//! - **存成单个 JSON 文件**而不是 SQLite。个人使用量级（几千条）下，
//!   JSON 的读写开销完全可忽略，而零额外依赖、用户能直接打开看内容。
//!   如果哪天条目上到十万级，再换 SQLite 不迟。
//! - **键用 SHA-256**。用 64 位哈希更快，但碰撞后会返回**错误的译文**——
//!   这个后果太严重，不值得省那点计算。
//! - **文件损坏不是错误**。解析失败就当空缓存，重新积累即可，
//!   绝不能因为缓存坏了就让工具不能用。

use crate::error::Result;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// 缓存条目数上限。超过后淘汰最旧的 20%。
///
/// 5000 条大约对应几百 KB 到几 MB 的 JSON，读写仍在毫秒级。
const MAX_ENTRIES: usize = 5000;

/// 缓存文件格式版本。**结构或译文语义**出现不兼容变化时递增；
/// 版本对不上就在 load 时整体丢弃，重新积累。
///
/// 缓存是可再生的（丢了只是重新走一次网络），所以这里选择「整体作废」
/// 而不是写迁移代码 —— 简单，且永远不会因为迁移逻辑出错而读到脏数据。
///
/// v1 → v2：修掉 MyMemory 的 Autodetect 通道会把原文当译文退回的问题。
/// 旧缓存里可能存着「译文 == 原文」这种脏数据，而键是 SHA-256、无法逐条甄别，
/// 只能整体作废 —— 否则修好的逻辑会被旧脏值长期挡住（命中缓存就不再请求了）。
///
/// v2 → v3：AI 后端开始处理 `to_if_same`（互翻）—— 旧 prompt 会把「中译中」
/// 的坏结果写进缓存（例如 Hunyuan-MT 把原句换个说法当译文返回）。
/// 同样的键在修好后应当产出不同的译文，按「译文语义变化即递增」的铁律整体作废。
///
/// v3 → v4：msedge / transmart / google 三家传统后端补上 `to_if_same` 处理
/// （此前它们对「中文进、目标也是中文」的请求**原样返回原文**，却把原文当译文
/// 写进了缓存）。旧缓存里同样存着「中译中」的坏结果，必须整体作废。
///
/// v4 → v5：AI 后端的 prompt 重写（互翻改成两条并列规则 + 正文包进 `<text>` 标签）。
/// 旧 prompt 会让模型对**英文输入**只做润色（原文退回）甚至复述指令 ——
/// 这些垃圾结果已经进了缓存，同样按铁律整体作废。
const FORMAT_VERSION: u32 = 5;

/// 单条缓存记录
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    /// 译文
    text: String,
    /// 写入时间（unix 秒），用于淘汰最旧条目
    at: u64,
}

/// 磁盘上的缓存文件结构
#[derive(Debug, Serialize, Deserialize)]
struct CacheFile {
    version: u32,
    entries: HashMap<String, Entry>,
}

/// 缓存句柄。
///
/// 生命周期：`load()` 读入内存 → 翻译过程中 `get`/`insert` → 结束时 `save()`。
/// 不在每次读写时碰磁盘，避免频繁 IO。
pub struct Cache {
    /// 缓存文件路径
    path: PathBuf,
    /// 内存中的条目表
    entries: HashMap<String, Entry>,
    /// 是否被修改过（没改就不用写盘）
    dirty: bool,
}

impl Cache {
    /// 定位缓存文件路径
    fn path() -> Result<PathBuf> {
        let dirs = directories::ProjectDirs::from("", "", "pory")
            .ok_or_else(|| crate::error::PoryError::Config("无法定位缓存目录".into()))?;
        Ok(dirs.cache_dir().join("cache.json"))
    }

    /// 从磁盘加载缓存。文件不存在或损坏都返回空缓存。
    pub fn load() -> Self {
        let Ok(path) = Self::path() else {
            // 定位不到目录（极端情况）：给一个空缓存，功能降级但不崩
            return Self {
                path: PathBuf::new(),
                entries: HashMap::new(),
                dirty: false,
            };
        };

        let entries = match std::fs::read_to_string(&path) {
            Ok(content) => match serde_json::from_str::<CacheFile>(&content) {
                // 版本不匹配 → 丢弃旧缓存（结构可能已变，不冒险解析）
                Ok(file) if file.version == FORMAT_VERSION => file.entries,
                Ok(_) => HashMap::new(),
                // 损坏 → 当空缓存。不报错，不让缓存问题阻断翻译。
                Err(_) => HashMap::new(),
            },
            Err(_) => HashMap::new(),
        };

        Self {
            path,
            entries,
            dirty: false,
        }
    }

    /// 查询缓存。未命中返回 None。
    ///
    /// 注意：这里**不负责统计命中率**。统计是调度器（Translator）的职责 ——
    /// 因为在后端回退链里，同一个块可能被多个后端各查一次缓存，
    /// 只有调度器知道最终结果到底来自缓存还是某个后端。
    pub fn get(&self, key: &str) -> Option<String> {
        self.entries.get(key).map(|e| e.text.clone())
    }

    /// 写入缓存
    pub fn insert(&mut self, key: String, text: String) {
        let at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        self.entries.insert(key, Entry { text, at });
        self.dirty = true;
    }

    /// 本次写盘专用的临时文件路径。
    ///
    /// 名字里带**进程号**，是为了让同时运行的两个 pory 进程各写各的临时文件。
    /// 这里踩过坑（2026-09-20 实测）：原先用固定的 `cache.json.tmp`，
    /// 两个进程并发收尾时，先完成的那个已经把临时文件 rename 走了，
    /// 后完成的再 rename 就找不到源文件，报
    /// `No such file or directory (os error 2)` —— 译文不受影响，
    /// 但那一次翻译的结果被丢掉了。20 实例并发可稳定偶发复现。
    ///
    /// ⚠️ 临时文件**必须和目标文件同目录**：这样才能保证 rename 落在同一个
    /// 文件系统上，是原子的。若图省事挪到系统 temp 目录，rename 可能跨设备，
    /// 退化成「复制 + 删除」，原子性就没了。别改这一点。
    fn tmp_path(&self) -> PathBuf {
        self.path
            .with_extension(format!("{}.tmp", std::process::id()))
    }

    /// 把缓存写回磁盘。
    ///
    /// 用「先写临时文件再改名」的原子写：即使写到一半崩溃，
    /// 原来的缓存文件也还是完整的，不会变成半截 JSON。
    pub fn save(&self) -> Result<()> {
        // 没改过就不写，省一次 IO
        if !self.dirty || self.path.as_os_str().is_empty() {
            return Ok(());
        }

        // 确保目录存在
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let mut entries = self.entries.clone();
        Self::evict_if_needed(&mut entries);

        let file = CacheFile {
            version: FORMAT_VERSION,
            entries,
        };

        let json = serde_json::to_string(&file)
            .map_err(|e| crate::error::PoryError::Cache(format!("缓存序列化失败：{e}")))?;

        // 原子写：临时文件 → rename
        let tmp = self.tmp_path();
        // 报错时把路径带上。否则只看到一句 "No such file or directory"，
        // 根本不知道是哪个文件 —— 为了定位这个 bug 绕了很多弯路。
        std::fs::write(&tmp, json).map_err(|e| {
            crate::error::PoryError::Cache(format!("缓存写入失败（{}）：{e}", tmp.display()))
        })?;

        // Windows 上 rename 不能覆盖已存在的文件，得先删目标
        if self.path.exists() {
            let _ = std::fs::remove_file(&self.path);
        }
        if let Err(e) = std::fs::rename(&tmp, &self.path) {
            // 失败就把自己的临时文件收走，别在缓存目录里留垃圾
            let _ = std::fs::remove_file(&tmp);
            return Err(crate::error::PoryError::Cache(format!(
                "缓存落盘失败（{} → {}）：{e}",
                tmp.display(),
                self.path.display()
            )));
        }

        Ok(())
    }

    /// 条目超上限时，淘汰最旧的 20%
    fn evict_if_needed(entries: &mut HashMap<String, Entry>) {
        if entries.len() <= MAX_ENTRIES {
            return;
        }

        // 按时间从新到旧排序，保留前 80%
        let mut by_time: Vec<(String, u64)> =
            entries.iter().map(|(k, e)| (k.clone(), e.at)).collect();
        by_time.sort_by_key(|(_, at)| std::cmp::Reverse(*at));

        let keep: HashSet<String> = by_time
            .into_iter()
            .take(MAX_ENTRIES * 4 / 5)
            .map(|(k, _)| k)
            .collect();

        entries.retain(|k, _| keep.contains(k));
    }

    /// 删除缓存文件（供 `--clear-cache` 使用）。返回被删除的路径。
    pub fn clear() -> Result<Option<PathBuf>> {
        let path = Self::path()?;
        if path.exists() {
            std::fs::remove_file(&path)
                .map_err(|e| crate::error::PoryError::Cache(format!("删除缓存失败：{e}")))?;
            Ok(Some(path))
        } else {
            Ok(None)
        }
    }
}

/// 生成缓存键。
///
/// 键由所有「影响翻译结果」的因素构成：
/// 后端名 + 后端附加标识（如 AI 的模型名）+ 源语言 + 目标语言 + 备用目标 + 原文。
/// 任何一项不同 → 键不同 → 缓存自然失效。这样就不会出现
/// 「换了模型却拿到旧模型的译文」这种脏数据。
///
/// `to_alt` 是「源语言 == 目标语言时改译的那个语言」（第一/第二语言互翻）。
/// 它**必须进键**：同一个 `auto → zh-CN` 请求，开着互翻会得到英文、关掉互翻会
/// 得到原文，两种结果的键若相同就会串味（关掉互翻后依然命中英文译文）。
/// 没有备用目标时传空串。
pub fn cache_key(
    backend: &str,
    backend_detail: &str,
    from: &str,
    to: &str,
    to_alt: &str,
    text: &str,
) -> String {
    let mut hasher = Sha256::new();
    // 用不可见的分隔符拼接，避免 "a|b" 和 "ab|" 撞成同一个键
    hasher.update(backend.as_bytes());
    hasher.update(b"\x1f");
    hasher.update(backend_detail.as_bytes());
    hasher.update(b"\x1f");
    hasher.update(from.as_bytes());
    hasher.update(b"\x1f");
    hasher.update(to.as_bytes());
    hasher.update(b"\x1f");
    hasher.update(to_alt.as_bytes());
    hasher.update(b"\x1f");
    hasher.update(text.as_bytes());

    // 转成十六进制字符串，方便当 JSON 的键
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 相同的输入产生相同的键() {
        let a = cache_key("mymemory", "", "en", "zh-CN", "", "hello");
        let b = cache_key("mymemory", "", "en", "zh-CN", "", "hello");
        assert_eq!(a, b);
    }

    #[test]
    fn 任何一项变化都会改变键() {
        let base = cache_key("mymemory", "", "en", "zh-CN", "", "hello");

        assert_ne!(base, cache_key("google", "", "en", "zh-CN", "", "hello"));
        assert_ne!(base, cache_key("ai", "gpt-4o", "en", "zh-CN", "", "hello"));
        assert_ne!(base, cache_key("mymemory", "", "ja", "zh-CN", "", "hello"));
        assert_ne!(base, cache_key("mymemory", "", "en", "ja", "", "hello"));
        // 备用目标也算「影响结果的因素」
        assert_ne!(
            base,
            cache_key("mymemory", "", "en", "zh-CN", "en", "hello")
        );
        assert_ne!(base, cache_key("mymemory", "", "en", "zh-CN", "", "world"));
    }

    #[test]
    fn 备用目标进键_否则开关互翻会串味() {
        // 同一个 auto → zh-CN 请求：开着互翻得到英文、关掉互翻得到原文。
        // 备用目标不进键的话，关掉互翻后会继续命中那份英文译文。
        let 关闭互翻 = cache_key("mymemory", "", "auto", "zh-CN", "", "你好世界");
        let 开启互翻 = cache_key("mymemory", "", "auto", "zh-CN", "en", "你好世界");
        assert_ne!(关闭互翻, 开启互翻);
    }

    #[test]
    fn 分隔符防止字段拼接歧义() {
        // 没有分隔符的话，这两组会撞成同一个字符串 "ab" + "c" vs "a" + "bc"
        let a = cache_key("ab", "c", "en", "zh", "", "x");
        let b = cache_key("a", "bc", "en", "zh", "", "x");
        assert_ne!(a, b);
    }

    #[test]
    fn 键是固定长度的十六进制() {
        let k = cache_key("mymemory", "", "en", "zh-CN", "", "hello");
        assert_eq!(k.len(), 64);
        assert!(k.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn 淘汰后条目数不超上限() {
        let mut entries = HashMap::new();
        for i in 0..(MAX_ENTRIES + 100) {
            entries.insert(
                format!("key{i}"),
                Entry {
                    text: format!("v{i}"),
                    // 时间递增，方便验证保留的是较新的
                    at: i as u64,
                },
            );
        }

        Cache::evict_if_needed(&mut entries);

        assert!(entries.len() <= MAX_ENTRIES);
        // 最新的一定还在
        assert!(entries.contains_key(&format!("key{}", MAX_ENTRIES + 99)));
        // 最旧的一定被淘汰了
        assert!(!entries.contains_key("key0"));
    }

    #[test]
    fn 未超上限时不淘汰() {
        let mut entries = HashMap::new();
        entries.insert(
            "a".to_string(),
            Entry {
                text: "x".to_string(),
                at: 1,
            },
        );
        Cache::evict_if_needed(&mut entries);
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn 临时文件与缓存文件同目录且带进程号() {
        let cache = Cache {
            path: PathBuf::from("/tmp/pory-demo/cache.json"),
            entries: HashMap::new(),
            dirty: false,
        };
        let tmp = cache.tmp_path();

        // 同目录 ⇒ 同文件系统 ⇒ rename 才是原子的。别改成系统 temp 目录。
        assert_eq!(tmp.parent(), cache.path.parent());
        assert_ne!(tmp, cache.path);
        // 带进程号 ⇒ 并发运行的两个 pory 不会抢同一个临时文件
        assert!(
            tmp.to_string_lossy()
                .contains(&std::process::id().to_string()),
            "临时文件名必须含进程号，否则并发进程会互相抢文件"
        );
    }

    #[test]
    fn 目录不存在时保存会自己创建目录() {
        // 这条行为曾经被我误判成 bug（以为 save 不创建父目录），实测是会创建的。
        // 加个测试钉住它，免得以后改坏。
        let dir = std::env::temp_dir().join(format!("pory-cache-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("cache.json");

        let mut cache = Cache {
            path: path.clone(),
            entries: HashMap::new(),
            dirty: false,
        };
        cache.insert("k".to_string(), "v".to_string());
        cache.save().expect("目录不存在时也应该保存成功");
        assert!(path.exists(), "save 应该创建父目录并写入缓存文件");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
